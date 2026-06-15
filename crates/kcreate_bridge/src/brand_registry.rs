//! Cross-project persistent brand-kit registry (workstream H5).
//!
//! Canva-style brand kits are stored *inside* a project (the
//! `brand_kits` table, surfaced through `document::brand_kit_*`). That
//! makes a kit local to one `.kstudio`. To let a user define a brand
//! once and reuse it across every project, this module persists kits to
//! a small on-disk registry next to the other registries the app keeps
//! (`thumbnails.rs`'s recent-projects roster, the plugin dir, the
//! template library):
//!
//! ```text
//! <registry-dir>/
//!   <kit-id>/
//!     kit.json        # StoredManifest: the BrandKit + asset refs
//!     <asset-id>.bin  # raw bytes for each referenced logo / font blob
//! ```
//!
//! The registry directory defaults to `$HOME/.kcreate/brand_kits` and
//! is overridable with `KCREATE_BRAND_KIT_DIR` (matching the
//! `KCREATE_RECENT_PROJECTS_FILE` / `KCREATE_PLUGIN_DIR` /
//! `KCREATE_TEMPLATE_DIR` overrides used elsewhere).
//!
//! A brand kit references its logo + fonts by asset id, and those ids
//! only resolve inside the project that minted them. So a saved kit
//! carries the *bytes* of each referenced asset as a sidecar `.bin`
//! file; when the kit is loaded into a (possibly different) project the
//! caller re-stores those bytes under fresh ids and relinks the kit —
//! exactly the round-trip `.kbrand` import/export already performs, but
//! persisted to the registry rather than a user-chosen archive file.
//!
//! This module is deliberately **stateless** (no `OnceLock` cache): save
//! / load / list / delete are infrequent, explicit user actions, and a
//! fresh `fs` read each time keeps the registry trivially consistent
//! with what's on disk (e.g. a kit saved by another window shows up
//! without cache invalidation) and makes per-test isolation a matter of
//! pointing the `_in` helpers at a temp dir.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use kcreate_core::project::BrandKit;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One referenced asset blob (the logo or a font file) carried
/// alongside a saved kit so the kit stays whole across projects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrandAssetBlob {
    /// The asset id the kit referenced at save time
    /// (`BrandKit::logo_asset_id` or a `FontRef::embedded_asset_id`).
    /// Used as the relink key when the kit is loaded back in.
    pub asset_id: Uuid,
    /// MIME type recorded so the blob is re-stored with the right type.
    pub mime: String,
    /// Raw asset bytes (held in memory; persisted to `<asset-id>.bin`).
    pub bytes: Vec<u8>,
}

/// A saved brand kit plus the bytes of every asset it references.
#[derive(Debug, Clone)]
pub struct BrandKitRecord {
    pub kit: BrandKit,
    pub assets: Vec<BrandAssetBlob>,
}

/// On-disk `kit.json` shape. Asset *bytes* live in sidecar files; the
/// manifest only records each asset's id + mime so a load knows what to
/// read back and how to re-store it.
#[derive(Debug, Serialize, Deserialize)]
struct StoredManifest {
    kit: BrandKit,
    assets: Vec<StoredAssetRef>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredAssetRef {
    asset_id: Uuid,
    mime: String,
}

/// Sidecar file name for an asset blob, relative to the kit directory.
fn asset_file_name(asset_id: Uuid) -> String {
    format!("{asset_id}.bin")
}

/// Root directory of the brand-kit registry. Honours
/// `KCREATE_BRAND_KIT_DIR`, else `$HOME/.kcreate/brand_kits` (falling
/// back to the system temp dir when `HOME` is unset, mirroring
/// `thumbnails::recent_projects_path`).
#[must_use]
pub fn registry_dir() -> PathBuf {
    if let Ok(s) = std::env::var("KCREATE_BRAND_KIT_DIR") {
        return PathBuf::from(s);
    }
    let base = std::env::var_os("HOME").map_or_else(std::env::temp_dir, PathBuf::from);
    base.join(".kcreate").join("brand_kits")
}

/// Persist `record` to the registry, replacing any existing entry with
/// the same kit id.
pub fn save_record(record: &BrandKitRecord) -> io::Result<()> {
    save_record_in(&registry_dir(), record)
}

/// Load the kit + its asset blobs for `id`, or `None` when absent.
pub fn load_record(id: Uuid) -> io::Result<Option<BrandKitRecord>> {
    load_record_in(&registry_dir(), id)
}

/// Every saved kit's metadata (no asset bytes), ordered by name then id
/// for a stable panel listing.
pub fn list_kits() -> io::Result<Vec<BrandKit>> {
    list_kits_in(&registry_dir())
}

/// Delete a saved kit and all its sidecar blobs. Returns `false` when
/// nothing was stored under `id`.
pub fn delete_kit(id: Uuid) -> io::Result<bool> {
    delete_kit_in(&registry_dir(), id)
}

// ---------------------------------------------------------------------------
// Directory-parameterised cores (pure I/O; unit-testable against a temp dir)
// ---------------------------------------------------------------------------

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // Distinct `.tmp` sibling then rename, so a crash mid-write never
    // leaves a torn file at the real path. The temp suffix is `.tmp`
    // (not `.bin`), so the sidecar-prune step never trips over it.
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn save_record_in(root: &Path, record: &BrandKitRecord) -> io::Result<()> {
    let dir = root.join(record.kit.id.to_string());
    fs::create_dir_all(&dir)?;

    // Write the asset sidecars first, then the manifest last: a manifest
    // on disk always implies its referenced blobs are already present.
    let mut refs = Vec::with_capacity(record.assets.len());
    let mut keep: HashSet<String> = HashSet::with_capacity(record.assets.len());
    for a in &record.assets {
        let fname = asset_file_name(a.asset_id);
        write_atomic(&dir.join(&fname), &a.bytes)?;
        keep.insert(fname);
        refs.push(StoredAssetRef {
            asset_id: a.asset_id,
            mime: a.mime.clone(),
        });
    }

    let manifest = StoredManifest {
        kit: record.kit.clone(),
        assets: refs,
    };
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_atomic(&dir.join("kit.json"), &bytes)?;

    // Drop any sidecars from a previous save that this kit no longer
    // references (e.g. the user replaced the logo or swapped a font),
    // so the kit directory never grows unbounded across re-saves.
    //
    // This is pure housekeeping that runs *after* the manifest + every
    // referenced blob are already durably on disk, so the save has
    // logically succeeded by this point. A transient failure while
    // pruning (e.g. the directory listing itself fails) must NOT be
    // reported back as a save failure — that would make the caller
    // (`document::brand_kit_registry_save`) surface a spurious error to
    // the UI for data that is actually safe, prompting a needless retry.
    // Hence the entire prune is best-effort, not just the per-file
    // removals inside it.
    let _ = prune_stale_sidecars(&dir, &keep);
    Ok(())
}

fn prune_stale_sidecars(dir: &Path, keep: &HashSet<String>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let is_bin = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("bin"));
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_bin && !keep.contains(&name) {
            // Best-effort: a failed prune is not fatal to the save.
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

fn load_record_in(root: &Path, id: Uuid) -> io::Result<Option<BrandKitRecord>> {
    let dir = root.join(id.to_string());
    let manifest_path = dir.join("kit.json");
    let bytes = match fs::read(&manifest_path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let manifest: StoredManifest = serde_json::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut assets = Vec::with_capacity(manifest.assets.len());
    for r in manifest.assets {
        let data = fs::read(dir.join(asset_file_name(r.asset_id)))?;
        assets.push(BrandAssetBlob {
            asset_id: r.asset_id,
            mime: r.mime,
            bytes: data,
        });
    }
    Ok(Some(BrandKitRecord {
        kit: manifest.kit,
        assets,
    }))
}

fn list_kits_in(root: &Path) -> io::Result<Vec<BrandKit>> {
    let mut kits = Vec::new();
    let read = match fs::read_dir(root) {
        Ok(r) => r,
        // An absent registry is simply "no saved kits yet".
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(kits),
        Err(e) => return Err(e),
    };
    for entry in read {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let manifest_path = entry.path().join("kit.json");
        // Skip unreadable / malformed entries rather than failing the
        // whole listing — one corrupt kit dir shouldn't hide the rest.
        let Ok(bytes) = fs::read(&manifest_path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<StoredManifest>(&bytes) else {
            continue;
        };
        kits.push(manifest.kit);
    }
    kits.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(kits)
}

fn delete_kit_in(root: &Path, id: Uuid) -> io::Result<bool> {
    let dir = root.join(id.to_string());
    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kcreate_core::node::RgbaColor;
    use kcreate_core::project::{FontRef, NamedColor};

    fn sample_kit() -> BrandKit {
        let mut kit = BrandKit::new("Acme");
        kit.colors = vec![
            NamedColor {
                name: "primary".into(),
                color: RgbaColor::new(0.1, 0.2, 0.9, 1.0),
            },
            NamedColor {
                name: "background".into(),
                color: RgbaColor::new(1.0, 1.0, 1.0, 1.0),
            },
        ];
        kit.fonts = vec![FontRef {
            family: "Inter".into(),
            weight: 400,
            italic: false,
            embedded_asset_id: None,
        }];
        kit
    }

    #[test]
    fn save_load_roundtrips_kit_and_asset_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut kit = sample_kit();
        let logo_id = Uuid::new_v4();
        let font_id = Uuid::new_v4();
        kit.logo_asset_id = Some(logo_id);
        kit.fonts[0].embedded_asset_id = Some(font_id);

        let record = BrandKitRecord {
            kit: kit.clone(),
            assets: vec![
                BrandAssetBlob {
                    asset_id: logo_id,
                    mime: "image/svg+xml".into(),
                    bytes: b"<svg/>".to_vec(),
                },
                BrandAssetBlob {
                    asset_id: font_id,
                    mime: "font/ttf".into(),
                    bytes: vec![0, 1, 2, 3, 4],
                },
            ],
        };
        save_record_in(dir.path(), &record).expect("save");

        let loaded = load_record_in(dir.path(), kit.id)
            .expect("load ok")
            .expect("present");
        assert_eq!(loaded.kit, kit);
        // Asset bytes survive the round-trip, keyed by their original id.
        let logo = loaded
            .assets
            .iter()
            .find(|a| a.asset_id == logo_id)
            .expect("logo blob");
        assert_eq!(logo.bytes, b"<svg/>");
        assert_eq!(logo.mime, "image/svg+xml");
        let font = loaded
            .assets
            .iter()
            .find(|a| a.asset_id == font_id)
            .expect("font blob");
        assert_eq!(font.bytes, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn load_missing_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(load_record_in(dir.path(), Uuid::new_v4())
            .expect("load ok")
            .is_none());
    }

    #[test]
    fn list_is_sorted_and_skips_non_kit_dirs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut zebra = sample_kit();
        zebra.name = "Zebra".into();
        let mut apple = sample_kit();
        apple.name = "Apple".into();
        save_record_in(
            dir.path(),
            &BrandKitRecord {
                kit: zebra,
                assets: vec![],
            },
        )
        .expect("save zebra");
        save_record_in(
            dir.path(),
            &BrandKitRecord {
                kit: apple,
                assets: vec![],
            },
        )
        .expect("save apple");
        // A stray non-kit directory must not break listing.
        fs::create_dir_all(dir.path().join("not-a-kit")).expect("mkdir");

        let kits = list_kits_in(dir.path()).expect("list");
        assert_eq!(kits.len(), 2);
        assert_eq!(kits[0].name, "Apple");
        assert_eq!(kits[1].name, "Zebra");
    }

    #[test]
    fn list_missing_dir_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist");
        assert!(list_kits_in(&missing).expect("list").is_empty());
    }

    #[test]
    fn resave_prunes_unreferenced_sidecars() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut kit = sample_kit();
        let old_logo = Uuid::new_v4();
        kit.logo_asset_id = Some(old_logo);
        save_record_in(
            dir.path(),
            &BrandKitRecord {
                kit: kit.clone(),
                assets: vec![BrandAssetBlob {
                    asset_id: old_logo,
                    mime: "image/png".into(),
                    bytes: vec![9, 9, 9],
                }],
            },
        )
        .expect("save v1");
        let kit_dir = dir.path().join(kit.id.to_string());
        assert!(kit_dir.join(asset_file_name(old_logo)).exists());

        // Replace the logo with a new asset; the old sidecar must go.
        let new_logo = Uuid::new_v4();
        kit.logo_asset_id = Some(new_logo);
        save_record_in(
            dir.path(),
            &BrandKitRecord {
                kit,
                assets: vec![BrandAssetBlob {
                    asset_id: new_logo,
                    mime: "image/png".into(),
                    bytes: vec![7, 7, 7],
                }],
            },
        )
        .expect("save v2");
        assert!(!kit_dir.join(asset_file_name(old_logo)).exists());
        assert!(kit_dir.join(asset_file_name(new_logo)).exists());
    }

    #[test]
    fn delete_removes_kit_and_reports_presence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let kit = sample_kit();
        save_record_in(
            dir.path(),
            &BrandKitRecord {
                kit: kit.clone(),
                assets: vec![],
            },
        )
        .expect("save");
        assert!(delete_kit_in(dir.path(), kit.id).expect("delete"));
        assert!(load_record_in(dir.path(), kit.id)
            .expect("load ok")
            .is_none());
        // Second delete reports "nothing there".
        assert!(!delete_kit_in(dir.path(), kit.id).expect("delete again"));
    }

    // A failure while pruning stale sidecars must not be reported as a
    // save failure: the manifest + every referenced blob are already
    // durably written by the time the prune runs. We force the prune's
    // directory listing to fail by pre-creating the kit dir without read
    // permission (write+exec only) — `read_dir` then errors with EACCES,
    // but the writes inside it still succeed. The save must return `Ok`
    // and the persisted record must load back intact once we restore
    // read access.
    #[cfg(unix)]
    #[test]
    fn save_succeeds_even_when_prune_listing_fails() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("tempdir");
        let kit = sample_kit();
        let kit_dir = root.path().join(kit.id.to_string());
        fs::create_dir_all(&kit_dir).expect("mkdir kit dir");
        // 0o300 = owner write+execute, NO read: writes/renames inside the
        // directory still work, but `fs::read_dir` (the first thing the
        // prune does) fails with a permission error.
        fs::set_permissions(&kit_dir, fs::Permissions::from_mode(0o300)).expect("chmod");

        let logo_id = Uuid::new_v4();
        let mut kit_with_logo = kit;
        kit_with_logo.logo_asset_id = Some(logo_id);
        let record = BrandKitRecord {
            kit: kit_with_logo.clone(),
            assets: vec![BrandAssetBlob {
                asset_id: logo_id,
                mime: "image/png".into(),
                bytes: vec![4, 2],
            }],
        };

        // Sanity-check the premise: the directory is genuinely unreadable
        // (so the prune's `read_dir` really does fail), yet the save still
        // returns Ok because the prune is best-effort.
        assert!(
            fs::read_dir(&kit_dir).is_err(),
            "kit dir should be unreadable for the test premise"
        );
        save_record_in(root.path(), &record).expect("save must succeed despite prune failure");

        // Restore read access and confirm the record persisted intact.
        fs::set_permissions(&kit_dir, fs::Permissions::from_mode(0o700)).expect("restore chmod");
        let loaded = load_record_in(root.path(), kit_with_logo.id)
            .expect("load ok")
            .expect("present");
        assert_eq!(loaded.kit, kit_with_logo);
        let logo = loaded
            .assets
            .iter()
            .find(|a| a.asset_id == logo_id)
            .expect("logo blob");
        assert_eq!(logo.bytes, vec![4, 2]);
    }
}
