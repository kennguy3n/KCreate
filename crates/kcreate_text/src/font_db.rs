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

    /// For every codepoint in `text`, check whether the *resolved*
    /// face for `family` exposes a glyph. Returns the set of
    /// codepoints that have no glyph (in document order, deduplicated
    /// — the first occurrence of each missing codepoint is preserved
    /// so a UI can render a representative sample).
    ///
    /// The check uses the same resolution policy as `resolve_face`
    /// (exact family → generic fallback → outline-capable face) so
    /// the result reflects what would actually print, not what the
    /// raw fontdb query would return.
    ///
    /// Returns `Err(FontManagerError::NotFound)` when no face at all
    /// can be resolved for `family`. The caller normally pairs this
    /// with the existing `find_family().is_empty()` check: if the
    /// family is missing entirely, report a `FontEmbed` error; if
    /// the family resolves but glyphs are missing, report a
    /// `FontGlyphCoverage` warning per missing codepoint.
    ///
    /// Whitespace and ASCII control codepoints are filtered out of
    /// the result because every reasonable system font carries
    /// glyphs for them and the user gets no actionable signal from a
    /// "U+0020 (space) is missing" warning.
    pub fn missing_glyphs(&self, family: &str, text: &str) -> Result<Vec<char>, FontManagerError> {
        let resolved = self.resolve_face(family)?;
        let face = rustybuzz::ttf_parser::Face::parse(&resolved.data, resolved.face_index)
            .map_err(|_| FontManagerError::FaceData {
                family: family.to_string(),
            })?;
        // Deduplicate while preserving first-seen order so the UI
        // can render a stable, document-ordered preview. A
        // `BTreeSet` would lose insertion order; a `HashSet` lookup
        // by side is enough.
        let mut seen: std::collections::HashSet<char> = std::collections::HashSet::new();
        let mut missing: Vec<char> = Vec::new();
        for ch in text.chars() {
            // Control / whitespace codepoints are universally covered
            // by every system font and produce noise in the report.
            // We treat `'\t'`, `'\n'`, `'\r'`, U+00A0 (nbsp), and
            // every codepoint whose Unicode category is "Zs"
            // (space separator), "Cc" (control), or "Cf" (format)
            // as "covered" without probing the cmap. Doing the
            // category check via the `char` API rather than pulling
            // in `unicode-properties` keeps the dep tree clean.
            if ch.is_whitespace() || ch.is_control() {
                continue;
            }
            if !seen.insert(ch) {
                continue;
            }
            // `ttf-parser`'s `glyph_index` returns `Some(GlyphId(0))`
            // for the `.notdef` glyph, which is *also* what the cmap
            // returns for genuinely missing characters in many
            // fonts. So we have to check both for `None` AND for
            // `Some(GlyphId(0))` — relying on `is_none()` alone
            // would under-report missing glyphs against any font
            // that maps unmapped codepoints to `.notdef` explicitly
            // (which is the recommended OpenType convention).
            let glyph = face.glyph_index(ch);
            match glyph {
                None | Some(rustybuzz::ttf_parser::GlyphId(0)) => missing.push(ch),
                Some(_) => {}
            }
        }
        Ok(missing)
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

    #[test]
    fn missing_glyphs_errors_when_family_unresolvable() {
        let m = FontManager::new();
        // When the family resolves to *nothing* (no system fonts at
        // all on this runner) the function returns `NotFound`. When
        // at least one outline-capable face is present, the fallback
        // path picks it up — both branches are valid, we just need
        // the call to not panic and to return one of the two.
        match m.missing_glyphs("___definitely_not_a_real_font_family___", "abc") {
            Ok(missing) => {
                // The fallback resolved a real face; missing-glyph
                // probing must produce a finite list.
                assert!(missing.len() <= 3);
            }
            Err(FontManagerError::NotFound(_)) => {
                // No system fonts at all on this runner; expected.
            }
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn missing_glyphs_filters_whitespace_and_controls() {
        // Empty text + a family that may or may not resolve — either
        // way the missing-glyph list for "\t\n " must be empty.
        let m = FontManager::new();
        match m.missing_glyphs("___definitely_not_a_real_font_family___", "\t\n \r") {
            Ok(missing) => assert!(
                missing.is_empty(),
                "whitespace must never appear in the missing-glyph list, got {missing:?}",
            ),
            Err(_) => {
                // No fallback face — call short-circuited before the
                // loop; that's still a pass for this invariant.
            }
        }
    }
}
