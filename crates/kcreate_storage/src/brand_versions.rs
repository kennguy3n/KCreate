//! Brand-kit versioning.
//!
//! Stores immutable snapshots of `BrandKit` values keyed by a
//! `version_id`. Each snapshot records the user-supplied
//! description so the version timeline in the UI doesn't read
//! like an opaque list of timestamps.

use chrono::{DateTime, Utc};
use kcreate_core::project::{BrandKit, NamedColor};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::schema::DatabaseError;

/// A single brand-kit snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrandKitVersion {
    pub version_id: Uuid,
    pub brand_kit_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub description: String,
    pub snapshot: BrandKit,
}

/// Structured diff between two brand-kit snapshots. Returned to
/// the UI so it can highlight what changed without re-running the
/// diff in TypeScript.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct BrandKitDiff {
    pub added_colors: Vec<NamedColor>,
    pub removed_colors: Vec<NamedColor>,
    pub changed_colors: Vec<ColorChange>,
    pub added_fonts: Vec<String>,
    pub removed_fonts: Vec<String>,
    pub spacing_changed: bool,
    pub export_rules_changed: bool,
    pub name_changed: Option<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ColorChange {
    pub name: String,
    pub before: NamedColor,
    pub after: NamedColor,
}

/// Persist a snapshot of the supplied brand kit.
pub fn save_brand_kit_version(
    conn: &Connection,
    brand_kit: &BrandKit,
    description: impl Into<String>,
) -> Result<BrandKitVersion, DatabaseError> {
    let version = BrandKitVersion {
        version_id: Uuid::new_v4(),
        brand_kit_id: brand_kit.id,
        timestamp: Utc::now(),
        description: description.into(),
        snapshot: brand_kit.clone(),
    };
    let snapshot_json = serde_json::to_string(&version.snapshot)
        .map_err(|e| DatabaseError::Sqlite(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;
    conn.execute(
        "INSERT INTO brand_kit_versions (version_id, brand_kit_id, timestamp, description, snapshot)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            version.version_id.to_string(),
            version.brand_kit_id.to_string(),
            version.timestamp.to_rfc3339(),
            version.description,
            snapshot_json,
        ],
    )?;
    Ok(version)
}

/// List versions for a brand kit, newest first.
pub fn list_brand_kit_versions(
    conn: &Connection,
    brand_kit_id: Uuid,
) -> Result<Vec<BrandKitVersion>, DatabaseError> {
    let mut stmt = conn.prepare(
        "SELECT version_id, brand_kit_id, timestamp, description, snapshot
         FROM brand_kit_versions
         WHERE brand_kit_id = ?1
         ORDER BY timestamp DESC",
    )?;
    let rows = stmt.query_map(params![brand_kit_id.to_string()], row_to_version)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Load a single version by id.
pub fn load_brand_kit_version(
    conn: &Connection,
    version_id: Uuid,
) -> Result<Option<BrandKitVersion>, DatabaseError> {
    let mut stmt = conn.prepare(
        "SELECT version_id, brand_kit_id, timestamp, description, snapshot
         FROM brand_kit_versions
         WHERE version_id = ?1",
    )?;
    let mut rows = stmt.query_map(params![version_id.to_string()], row_to_version)?;
    match rows.next() {
        Some(Ok(v)) => Ok(Some(v)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

/// "Restore" is a load + caller-side overwrite: this function does
/// not mutate the brand kit table itself (that's the bridge layer's
/// responsibility, since brand kits live in `brand_kits` which is
/// also keyed by id and ordering of writes needs the operation log
/// update). Returns the snapshot ready to be written.
pub fn restore_brand_kit_version(
    conn: &Connection,
    version_id: Uuid,
) -> Result<BrandKit, DatabaseError> {
    let version = load_brand_kit_version(conn, version_id)?
        .ok_or_else(|| DatabaseError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;
    Ok(version.snapshot)
}

/// Compute a structured diff between two snapshots.
#[must_use]
pub fn diff_brand_kit_versions(before: &BrandKit, after: &BrandKit) -> BrandKitDiff {
    let mut diff = BrandKitDiff::default();
    if before.name != after.name {
        diff.name_changed = Some((before.name.clone(), after.name.clone()));
    }
    // Colours: match by name (case-sensitive). `removed` = present
    // in before but not in after; `added` = vice versa; `changed`
    // = present in both with a different value.
    let before_by_name: std::collections::HashMap<&str, &NamedColor> =
        before.colors.iter().map(|c| (c.name.as_str(), c)).collect();
    let after_by_name: std::collections::HashMap<&str, &NamedColor> =
        after.colors.iter().map(|c| (c.name.as_str(), c)).collect();
    for (name, before_c) in &before_by_name {
        match after_by_name.get(name) {
            None => diff.removed_colors.push((**before_c).clone()),
            Some(after_c) if after_c != before_c => diff.changed_colors.push(ColorChange {
                name: (*name).to_string(),
                before: (**before_c).clone(),
                after: (**after_c).clone(),
            }),
            _ => {}
        }
    }
    for (name, after_c) in &after_by_name {
        if !before_by_name.contains_key(name) {
            diff.added_colors.push((**after_c).clone());
        }
    }
    // Sort outputs so the diff is deterministic.
    diff.added_colors.sort_by(|a, b| a.name.cmp(&b.name));
    diff.removed_colors.sort_by(|a, b| a.name.cmp(&b.name));
    diff.changed_colors.sort_by(|a, b| a.name.cmp(&b.name));

    // Fonts: match by family. Weight/italic differences count as
    // changed; but a removed-and-readded pair is rare so we model
    // it as added + removed for simplicity.
    let before_families: std::collections::HashSet<&str> =
        before.fonts.iter().map(|f| f.family.as_str()).collect();
    let after_families: std::collections::HashSet<&str> =
        after.fonts.iter().map(|f| f.family.as_str()).collect();
    for f in &before.fonts {
        if !after_families.contains(f.family.as_str()) {
            diff.removed_fonts.push(f.family.clone());
        }
    }
    for f in &after.fonts {
        if !before_families.contains(f.family.as_str()) {
            diff.added_fonts.push(f.family.clone());
        }
    }
    diff.added_fonts.sort();
    diff.removed_fonts.sort();

    diff.spacing_changed = before.spacing_scale != after.spacing_scale;
    diff.export_rules_changed = before.export_rules != after.export_rules;
    diff
}

fn row_to_version(row: &rusqlite::Row<'_>) -> rusqlite::Result<BrandKitVersion> {
    let version_id: String = row.get(0)?;
    let brand_kit_id: String = row.get(1)?;
    let timestamp: String = row.get(2)?;
    let description: String = row.get(3)?;
    let snapshot_json: String = row.get(4)?;
    let snapshot: BrandKit = serde_json::from_str(&snapshot_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(BrandKitVersion {
        version_id: parse_uuid(&version_id)?,
        brand_kit_id: parse_uuid(&brand_kit_id)?,
        timestamp: chrono::DateTime::parse_from_rfc3339(&timestamp)
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?
            .with_timezone(&chrono::Utc),
        description,
        snapshot,
    })
}

fn parse_uuid(s: &str) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Database;
    use kcreate_core::node::RgbaColor;
    use kcreate_core::project::BrandKit;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, Database) {
        let tmp = TempDir::new().expect("tmp");
        let db = Database::open(tmp.path().join("brand.db")).expect("open");
        (tmp, db)
    }

    fn brand_kit_with_colors(name: &str, colors: &[(&str, [u8; 4])]) -> BrandKit {
        let mut kit = BrandKit::new(name);
        for (n, rgba) in colors {
            kit.colors.push(NamedColor {
                name: (*n).to_string(),
                color: RgbaColor::new(
                    f32::from(rgba[0]) / 255.0,
                    f32::from(rgba[1]) / 255.0,
                    f32::from(rgba[2]) / 255.0,
                    f32::from(rgba[3]) / 255.0,
                ),
            });
        }
        kit
    }

    #[test]
    fn save_list_roundtrip() {
        let (_tmp, db) = fresh_db();
        let kit = brand_kit_with_colors("Acme", &[("primary", [10, 20, 30, 255])]);
        let v = save_brand_kit_version(db.conn(), &kit, "initial").expect("save");
        let listed = list_brand_kit_versions(db.conn(), kit.id).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].version_id, v.version_id);
        assert_eq!(listed[0].snapshot.name, "Acme");
    }

    #[test]
    fn list_is_newest_first() {
        let (_tmp, db) = fresh_db();
        let kit = brand_kit_with_colors("Acme", &[]);
        let v1 = save_brand_kit_version(db.conn(), &kit, "v1").expect("save");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let v2 = save_brand_kit_version(db.conn(), &kit, "v2").expect("save");
        let listed = list_brand_kit_versions(db.conn(), kit.id).expect("list");
        assert_eq!(listed[0].version_id, v2.version_id);
        assert_eq!(listed[1].version_id, v1.version_id);
    }

    #[test]
    fn diff_detects_added_removed_changed_colors() {
        let before = brand_kit_with_colors(
            "Acme",
            &[("primary", [10, 20, 30, 255]), ("accent", [200, 0, 0, 255])],
        );
        let after = brand_kit_with_colors(
            "Acme",
            &[
                ("primary", [10, 20, 31, 255]),      // changed
                ("secondary", [100, 100, 100, 255]), // added
            ],
        );
        let diff = diff_brand_kit_versions(&before, &after);
        assert_eq!(diff.added_colors.len(), 1);
        assert_eq!(diff.added_colors[0].name, "secondary");
        assert_eq!(diff.removed_colors.len(), 1);
        assert_eq!(diff.removed_colors[0].name, "accent");
        assert_eq!(diff.changed_colors.len(), 1);
        assert_eq!(diff.changed_colors[0].name, "primary");
    }

    #[test]
    fn diff_detects_name_change() {
        let before = brand_kit_with_colors("Old", &[]);
        let after = brand_kit_with_colors("New", &[]);
        // The diff matches by name so we need to pin the id of after
        // to match before to avoid creating two separate kits.
        let mut after = after;
        after.id = before.id;
        let diff = diff_brand_kit_versions(&before, &after);
        assert_eq!(diff.name_changed, Some(("Old".into(), "New".into())));
    }

    #[test]
    fn restore_returns_stored_snapshot() {
        let (_tmp, db) = fresh_db();
        let kit = brand_kit_with_colors("Acme", &[("primary", [1, 2, 3, 255])]);
        let v = save_brand_kit_version(db.conn(), &kit, "init").expect("save");
        let restored = restore_brand_kit_version(db.conn(), v.version_id).expect("restore");
        assert_eq!(restored.name, "Acme");
        assert_eq!(restored.colors.len(), 1);
    }
}
