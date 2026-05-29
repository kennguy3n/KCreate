//! Persistence for ruler guides (Phase 9 Task 21).
//!
//! Guides are stored per-page in the project DB so they survive
//! close / reopen and round-trip cleanly via collab. They are
//! NOT part of the node graph — they're purely a UI affordance
//! and don't participate in the operation log directly, although
//! the bridge layer records a "guide.create" / "guide.delete"
//! audit event for each mutation.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::schema::DatabaseError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GuideOrientation {
    Horizontal,
    Vertical,
}

impl GuideOrientation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "horizontal" => Some(Self::Horizontal),
            "vertical" => Some(Self::Vertical),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Guide {
    pub id: Uuid,
    pub page_id: Uuid,
    pub orientation: GuideOrientation,
    /// For `Horizontal`: y coordinate (in page-local px). For
    /// `Vertical`: x coordinate.
    pub position: f64,
    /// `#rrggbb` color used for the on-canvas overlay.
    pub color: String,
    /// When set, the user can't accidentally drag this guide.
    pub locked: bool,
    pub created_at: DateTime<Utc>,
}

/// Insert (or upsert on conflict — same id replaces the row) a guide.
pub fn upsert_guide(conn: &Connection, g: &Guide) -> Result<(), DatabaseError> {
    conn.execute(
        "INSERT INTO guides
            (id, page_id, orientation, position, color, locked, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
            page_id = excluded.page_id,
            orientation = excluded.orientation,
            position = excluded.position,
            color = excluded.color,
            locked = excluded.locked",
        params![
            g.id.to_string(),
            g.page_id.to_string(),
            g.orientation.as_str(),
            g.position,
            g.color,
            i64::from(g.locked),
            g.created_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn delete_guide(conn: &Connection, id: Uuid) -> Result<bool, DatabaseError> {
    let n = conn.execute("DELETE FROM guides WHERE id = ?1", params![id.to_string()])?;
    Ok(n > 0)
}

pub fn delete_all_for_page(conn: &Connection, page_id: Uuid) -> Result<u64, DatabaseError> {
    let n = conn.execute(
        "DELETE FROM guides WHERE page_id = ?1",
        params![page_id.to_string()],
    )?;
    Ok(n as u64)
}

pub fn list_for_page(conn: &Connection, page_id: Uuid) -> Result<Vec<Guide>, DatabaseError> {
    let mut stmt = conn.prepare(
        "SELECT id, page_id, orientation, position, color, locked, created_at
         FROM guides
         WHERE page_id = ?1
         ORDER BY orientation ASC, position ASC",
    )?;
    let rows = stmt.query_map(params![page_id.to_string()], row_to_guide)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn list_all(conn: &Connection) -> Result<Vec<Guide>, DatabaseError> {
    let mut stmt = conn.prepare(
        "SELECT id, page_id, orientation, position, color, locked, created_at
         FROM guides
         ORDER BY page_id ASC, orientation ASC, position ASC",
    )?;
    let rows = stmt.query_map([], row_to_guide)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn row_to_guide(row: &rusqlite::Row<'_>) -> rusqlite::Result<Guide> {
    let id_s: String = row.get(0)?;
    let page_s: String = row.get(1)?;
    let orient_s: String = row.get(2)?;
    let position: f64 = row.get(3)?;
    let color: String = row.get(4)?;
    let locked_i: i64 = row.get(5)?;
    let created_s: String = row.get(6)?;
    let id = Uuid::parse_str(&id_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let page_id = Uuid::parse_str(&page_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let orientation = GuideOrientation::parse(&orient_s).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unexpected orientation '{orient_s}'"),
            )),
        )
    })?;
    let created_at = DateTime::parse_from_rfc3339(&created_s)
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e))
        })?
        .with_timezone(&Utc);
    Ok(Guide {
        id,
        page_id,
        orientation,
        position,
        color,
        locked: locked_i != 0,
        created_at,
    })
}

/// Grid settings for a single artboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GridSettings {
    pub artboard_id: Uuid,
    pub enabled: bool,
    pub spacing: f64,
    pub subdivisions: u32,
    pub color: String,
}

impl GridSettings {
    pub fn default_for(artboard_id: Uuid) -> Self {
        Self {
            artboard_id,
            enabled: false,
            spacing: 16.0,
            subdivisions: 2,
            color: "#cccccc".to_string(),
        }
    }
}

pub fn upsert_grid_settings(conn: &Connection, s: &GridSettings) -> Result<(), DatabaseError> {
    conn.execute(
        "INSERT INTO grid_settings (artboard_id, enabled, spacing, subdivisions, color, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(artboard_id) DO UPDATE SET
            enabled = excluded.enabled,
            spacing = excluded.spacing,
            subdivisions = excluded.subdivisions,
            color = excluded.color,
            updated_at = excluded.updated_at",
        params![
            s.artboard_id.to_string(),
            i64::from(s.enabled),
            s.spacing,
            i64::from(s.subdivisions),
            s.color,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn load_grid_settings(
    conn: &Connection,
    artboard_id: Uuid,
) -> Result<Option<GridSettings>, DatabaseError> {
    let mut stmt = conn.prepare(
        "SELECT artboard_id, enabled, spacing, subdivisions, color
         FROM grid_settings
         WHERE artboard_id = ?1",
    )?;
    let mut rows = stmt.query_map(params![artboard_id.to_string()], |r| {
        let id_s: String = r.get(0)?;
        let enabled_i: i64 = r.get(1)?;
        let spacing: f64 = r.get(2)?;
        let subdivs: i64 = r.get(3)?;
        let color: String = r.get(4)?;
        let id = Uuid::parse_str(&id_s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
        Ok(GridSettings {
            artboard_id: id,
            enabled: enabled_i != 0,
            spacing,
            subdivisions: subdivs.max(0) as u32,
            color,
        })
    })?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;

    fn open_in_memory() -> Database {
        // `Database::open` takes a real path, so back the test with a
        // tempdir that the test owner keeps alive for the duration
        // of the test (no `mem::forget` — that would leak the dir
        // across the entire test binary).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project.db");
        let db = Database::open(path).unwrap();
        // `Database` borrows from the connection it owns, not from
        // the directory — but for safety we leak `dir` so its
        // backing files exist for the test's lifetime.
        std::mem::forget(dir);
        db
    }

    #[test]
    fn upsert_and_list_guides_round_trips() {
        let db = open_in_memory();
        let page = Uuid::new_v4();
        let g1 = Guide {
            id: Uuid::new_v4(),
            page_id: page,
            orientation: GuideOrientation::Horizontal,
            position: 100.5,
            color: "#0099ff".to_string(),
            locked: false,
            created_at: Utc::now(),
        };
        let g2 = Guide {
            id: Uuid::new_v4(),
            page_id: page,
            orientation: GuideOrientation::Vertical,
            position: 250.0,
            color: "#ff0099".to_string(),
            locked: true,
            created_at: Utc::now(),
        };
        upsert_guide(db.conn(), &g1).unwrap();
        upsert_guide(db.conn(), &g2).unwrap();
        let listed = list_for_page(db.conn(), page).unwrap();
        assert_eq!(listed.len(), 2);
        // Sorted: Horizontal first.
        assert_eq!(listed[0].id, g1.id);
        assert_eq!(listed[1].id, g2.id);
        assert!(listed[1].locked);
    }

    #[test]
    fn delete_guide_returns_true_only_first_time() {
        let db = open_in_memory();
        let g = Guide {
            id: Uuid::new_v4(),
            page_id: Uuid::new_v4(),
            orientation: GuideOrientation::Horizontal,
            position: 10.0,
            color: "#000000".to_string(),
            locked: false,
            created_at: Utc::now(),
        };
        upsert_guide(db.conn(), &g).unwrap();
        assert!(delete_guide(db.conn(), g.id).unwrap());
        assert!(!delete_guide(db.conn(), g.id).unwrap());
    }

    #[test]
    fn grid_settings_round_trip() {
        let db = open_in_memory();
        let artboard = Uuid::new_v4();
        assert!(load_grid_settings(db.conn(), artboard).unwrap().is_none());
        let settings = GridSettings {
            artboard_id: artboard,
            enabled: true,
            spacing: 24.0,
            subdivisions: 4,
            color: "#888888".to_string(),
        };
        upsert_grid_settings(db.conn(), &settings).unwrap();
        let loaded = load_grid_settings(db.conn(), artboard).unwrap().unwrap();
        assert_eq!(loaded, settings);
        // Update preserves the row.
        let s2 = GridSettings {
            spacing: 32.0,
            ..settings
        };
        upsert_grid_settings(db.conn(), &s2).unwrap();
        let loaded = load_grid_settings(db.conn(), artboard).unwrap().unwrap();
        assert_eq!(loaded.spacing, 32.0);
    }
}
