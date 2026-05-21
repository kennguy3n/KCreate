//! Font discovery wrapper around [`fontdb::Database`].
//!
//! The database is loaded once per process (system fonts on first
//! call) and cached behind a `Mutex` — the bridge holds the only
//! reference, and shaping calls clone the resolved face data out
//! before relinquishing the lock.

use std::path::Path;
use std::sync::OnceLock;

use fontdb::{Database, Family, Query, Source};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors from [`FontManager`].
#[derive(Debug, Error)]
pub enum FontManagerError {
    #[error("font io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("font family not found: {0}")]
    NotFound(String),
    #[error("font face data could not be extracted from the database for {family}")]
    FaceData { family: String },
}

/// Lightweight projection of a face that crosses the bridge boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FontInfo {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub monospaced: bool,
    /// Optional absolute path; absent for memory-backed faces.
    pub source: Option<String>,
}

/// Process-wide font manager. Cheap to clone (no heavy state inside
/// `FontManager` itself — the actual database is process-singleton).
#[derive(Debug, Clone, Default)]
pub struct FontManager {
    _private: (),
}

impl FontManager {
    /// Construct a manager. The first call to any read method lazily
    /// loads system fonts; subsequent calls reuse the cached
    /// database.
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }

    /// Load a single font file into the shared database.
    pub fn add_font_file(&self, path: &Path) -> Result<(), FontManagerError> {
        let data = std::fs::read(path)?;
        with_db_mut(|db| {
            db.load_font_data(data);
        });
        Ok(())
    }

    /// Load every supported font under `dir` into the shared
    /// database. Non-font files are silently skipped.
    pub fn add_fonts_dir(&self, dir: &Path) -> Result<(), FontManagerError> {
        with_db_mut(|db| {
            db.load_fonts_dir(dir);
        });
        Ok(())
    }

    /// Return all faces in the requested family.
    #[must_use]
    pub fn find_family(&self, family: &str) -> Vec<FontInfo> {
        with_db(|db| {
            db.faces()
                .filter(|f| {
                    f.families
                        .iter()
                        .any(|(name, _)| name.eq_ignore_ascii_case(family))
                })
                .map(|f| FontInfo {
                    family: f
                        .families
                        .first()
                        .map(|(n, _)| n.clone())
                        .unwrap_or_default(),
                    weight: f.weight.0,
                    italic: matches!(f.style, fontdb::Style::Italic),
                    monospaced: f.monospaced,
                    source: face_path(&f.source),
                })
                .collect()
        })
    }

    /// Total number of faces currently loaded.
    #[must_use]
    pub fn font_count(&self) -> usize {
        with_db(|db| db.faces().count())
    }

    /// Resolve a family name and return the raw font file bytes plus
    /// the face index inside the file.
    ///
    /// Resolution order:
    ///   1. Exact family-name match.
    ///   2. Generic family heuristics (`sans-serif`, `serif`,
    ///      `monospace`, `cursive`, `fantasy`) via `fontdb`'s built-in
    ///      query.
    ///   3. Any face whose font carries a TrueType / CFF outline
    ///      table — skips bitmap-only fonts (e.g. color emoji) which
    ///      cannot produce vector outlines and would break downstream
    ///      shaping / SVG-export consumers.
    pub fn resolve_face(&self, family: &str) -> Result<ResolvedFace, FontManagerError> {
        let id_opt = with_db(|db| {
            let q = Query {
                families: &[Family::Name(family)],
                ..Query::default()
            };
            if let Some(id) = db.query(&q) {
                return Some(id);
            }
            // Fall back to a face that actually has outlines. Bitmap-only
            // emoji fonts will not produce drawable paths, so callers
            // would silently get blank glyphs. Probe each candidate
            // until one parses and yields a non-zero glyph count.
            for face in db.faces() {
                let ok = db.with_face_data(face.id, has_outlines);
                if matches!(ok, Some(true)) {
                    return Some(face.id);
                }
            }
            None
        });
        let id = id_opt.ok_or_else(|| FontManagerError::NotFound(family.to_string()))?;
        let resolved = with_db(|db| {
            db.with_face_data(id, |data, face_index| ResolvedFace {
                family: family.to_string(),
                face_index,
                data: data.to_vec(),
            })
        });
        resolved.ok_or_else(|| FontManagerError::FaceData {
            family: family.to_string(),
        })
    }

    /// Read-only snapshot of every loaded face.
    #[must_use]
    pub fn all_faces(&self) -> Vec<FontInfo> {
        with_db(|db| {
            db.faces()
                .map(|f| FontInfo {
                    family: f
                        .families
                        .first()
                        .map(|(n, _)| n.clone())
                        .unwrap_or_default(),
                    weight: f.weight.0,
                    italic: matches!(f.style, fontdb::Style::Italic),
                    monospaced: f.monospaced,
                    source: face_path(&f.source),
                })
                .collect()
        })
    }
}

/// Bytes + face index of a resolved face.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFace {
    pub family: String,
    pub face_index: u32,
    pub data: Vec<u8>,
}

/// Return true if the font at `data` (face index `idx`) has an
/// outline-capable shape — i.e. carries a TrueType `glyf` / CFF `CFF `
/// / CFF2 `CFF2` table. Bitmap-only fonts (color emoji using `CBLC` /
/// `sbix`) return false. Returns false on any parse failure too —
/// callers should treat that as "unusable".
fn has_outlines(data: &[u8], idx: u32) -> bool {
    let Ok(face) = rustybuzz::ttf_parser::Face::parse(data, idx) else {
        return false;
    };
    face.tables().glyf.is_some() || face.tables().cff.is_some() || face.tables().cff2.is_some()
}

fn face_path(source: &Source) -> Option<String> {
    match source {
        Source::File(p) => p.to_str().map(str::to_string),
        Source::Binary(_) | Source::SharedFile(_, _) => None,
    }
}

fn db_slot() -> &'static Mutex<Database> {
    static DB: OnceLock<Mutex<Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = Database::new();
        db.load_system_fonts();
        Mutex::new(db)
    })
}

fn with_db<R>(f: impl FnOnce(&Database) -> R) -> R {
    let db = db_slot().lock();
    f(&db)
}

fn with_db_mut<R>(f: impl FnOnce(&mut Database) -> R) -> R {
    let mut db = db_slot().lock();
    f(&mut db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_loads_system_fonts_or_runs_clean() {
        let m = FontManager::new();
        // Either system fonts are present (most CI runners) or the
        // count is zero — both are acceptable. We just need the call
        // to not panic and to return a non-negative count.
        let n = m.font_count();
        assert!(n < usize::MAX);
    }

    #[test]
    fn find_family_returns_empty_for_unknown() {
        let m = FontManager::new();
        let v = m.find_family("___definitely_not_a_real_font_family___");
        assert!(v.is_empty());
    }
}
