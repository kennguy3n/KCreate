//! Annotation persistence layer.
//!
//! Annotations are stored in the project DB in their own table.
//! This module owns the read / write SQL, leaving the bridge layer
//! to handle broadcasting (collab) and the renderer overlay.

use kcreate_core::annotation::{Annotation, AnnotationFilter, AnnotationPosition};
use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::schema::DatabaseError;

/// Insert or update an annotation (`INSERT … ON CONFLICT … DO
/// UPDATE`). Used by both the local-edit path and the collab-
/// inbound path; for collab, the remote peer wins on conflict
/// because their version's timestamp is the authoritative one.
pub fn upsert_annotation(conn: &Connection, ann: &Annotation) -> Result<(), DatabaseError> {
    conn.execute(
        "INSERT INTO annotations
            (id, page_id, author_peer_id, author_name,
             position_x, position_y, text, timestamp, resolved, thread_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
            page_id = excluded.page_id,
            author_peer_id = excluded.author_peer_id,
            author_name = excluded.author_name,
            position_x = excluded.position_x,
            position_y = excluded.position_y,
            text = excluded.text,
            timestamp = excluded.timestamp,
            resolved = excluded.resolved,
            thread_id = excluded.thread_id",
        params![
            ann.id.to_string(),
            ann.page_id.to_string(),
            ann.author_peer_id,
            ann.author_name,
            ann.position.x,
            ann.position.y,
            ann.text,
            ann.timestamp.to_rfc3339(),
            i64::from(ann.resolved),
            ann.thread_id.map(|id| id.to_string()),
        ],
    )?;
    Ok(())
}

/// Delete a single annotation by id. Replies that referenced this
/// annotation as their thread root are NOT cascaded — they remain
/// in place but become "orphaned replies" the UI presents as a
/// flat list under the page heading.
pub fn delete_annotation(conn: &Connection, id: Uuid) -> Result<bool, DatabaseError> {
    let n = conn.execute(
        "DELETE FROM annotations WHERE id = ?1",
        params![id.to_string()],
    )?;
    Ok(n > 0)
}

/// Mark an annotation resolved / unresolved. Returns the new
/// state when found, or `None` when the id is unknown.
pub fn set_resolved(
    conn: &Connection,
    id: Uuid,
    resolved: bool,
) -> Result<Option<bool>, DatabaseError> {
    let n = conn.execute(
        "UPDATE annotations SET resolved = ?1 WHERE id = ?2",
        params![i64::from(resolved), id.to_string()],
    )?;
    if n == 0 {
        return Ok(None);
    }
    Ok(Some(resolved))
}

/// List annotations for a single page, applying the supplied
/// filter and returning rows sorted by timestamp (oldest first
/// so the UI can render threads chronologically).
pub fn list_for_page(
    conn: &Connection,
    page_id: Uuid,
    filter: AnnotationFilter,
) -> Result<Vec<Annotation>, DatabaseError> {
    let mut stmt = conn.prepare(
        "SELECT id, page_id, author_peer_id, author_name,
                position_x, position_y, text, timestamp, resolved, thread_id
         FROM annotations
         WHERE page_id = ?1
         ORDER BY timestamp ASC",
    )?;
    let rows = stmt.query_map(params![page_id.to_string()], row_to_annotation)?;
    let mut out = Vec::new();
    for r in rows {
        let ann = r?;
        if filter.matches(&ann) {
            out.push(ann);
        }
    }
    Ok(out)
}

/// List every annotation across every page. Used by the audit
/// trail export.
pub fn list_all(
    conn: &Connection,
    filter: AnnotationFilter,
) -> Result<Vec<Annotation>, DatabaseError> {
    let mut stmt = conn.prepare(
        "SELECT id, page_id, author_peer_id, author_name,
                position_x, position_y, text, timestamp, resolved, thread_id
         FROM annotations
         ORDER BY timestamp ASC",
    )?;
    let rows = stmt.query_map([], row_to_annotation)?;
    let mut out = Vec::new();
    for r in rows {
        let ann = r?;
        if filter.matches(&ann) {
            out.push(ann);
        }
    }
    Ok(out)
}

fn row_to_annotation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Annotation> {
    let id: String = row.get(0)?;
    let page_id: String = row.get(1)?;
    let author_peer_id: String = row.get(2)?;
    let author_name: String = row.get(3)?;
    let position_x: f64 = row.get(4)?;
    let position_y: f64 = row.get(5)?;
    let text: String = row.get(6)?;
    let timestamp: String = row.get(7)?;
    let resolved: i64 = row.get(8)?;
    let thread_id: Option<String> = row.get(9)?;
    Ok(Annotation {
        id: parse_uuid(&id)?,
        page_id: parse_uuid(&page_id)?,
        author_peer_id,
        author_name,
        position: AnnotationPosition {
            x: position_x,
            y: position_y,
        },
        text,
        timestamp: chrono::DateTime::parse_from_rfc3339(&timestamp)
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    7,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?
            .with_timezone(&chrono::Utc),
        resolved: resolved != 0,
        thread_id: thread_id.as_deref().map(parse_uuid).transpose()?,
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
    use tempfile::TempDir;
    use uuid::Uuid;

    fn fresh_db() -> (TempDir, Database) {
        let tmp = TempDir::new().expect("tmp");
        let db = Database::open(tmp.path().join("test.db")).expect("open");
        (tmp, db)
    }

    #[test]
    fn upsert_and_list_round_trip() {
        let (_tmp, db) = fresh_db();
        let page = Uuid::new_v4();
        let ann = Annotation::new(
            page,
            "peer-1",
            "Alice",
            AnnotationPosition { x: 10.0, y: 20.0 },
            "Hi",
        );
        upsert_annotation(db.conn(), &ann).expect("upsert");
        let found = list_for_page(db.conn(), page, AnnotationFilter::all()).expect("list");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, ann.id);
        assert_eq!(found[0].text, "Hi");
        assert!((found[0].position.x - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn upsert_overwrites_existing() {
        let (_tmp, db) = fresh_db();
        let page = Uuid::new_v4();
        let mut ann = Annotation::new(
            page,
            "peer-1",
            "Alice",
            AnnotationPosition { x: 0.0, y: 0.0 },
            "first",
        );
        upsert_annotation(db.conn(), &ann).expect("upsert");
        ann.text = "second".into();
        upsert_annotation(db.conn(), &ann).expect("upsert");
        let found = list_for_page(db.conn(), page, AnnotationFilter::all()).expect("list");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "second");
    }

    #[test]
    fn delete_returns_false_for_unknown() {
        let (_tmp, db) = fresh_db();
        assert!(!delete_annotation(db.conn(), Uuid::new_v4()).expect("delete"));
    }

    #[test]
    fn set_resolved_toggles() {
        let (_tmp, db) = fresh_db();
        let page = Uuid::new_v4();
        let ann = Annotation::new(
            page,
            "peer-1",
            "Alice",
            AnnotationPosition { x: 0.0, y: 0.0 },
            "x",
        );
        upsert_annotation(db.conn(), &ann).expect("upsert");
        set_resolved(db.conn(), ann.id, true).expect("resolve");
        let after = list_for_page(db.conn(), page, AnnotationFilter::all()).expect("list");
        assert!(after[0].resolved);
        let unres =
            list_for_page(db.conn(), page, AnnotationFilter::unresolved_only()).expect("list");
        assert!(unres.is_empty(), "resolved annotation should be filtered");
    }

    #[test]
    fn list_for_page_filters_other_pages() {
        let (_tmp, db) = fresh_db();
        let page_a = Uuid::new_v4();
        let page_b = Uuid::new_v4();
        let ann_a = Annotation::new(
            page_a,
            "p1",
            "A",
            AnnotationPosition { x: 0.0, y: 0.0 },
            "a",
        );
        let ann_b = Annotation::new(
            page_b,
            "p1",
            "A",
            AnnotationPosition { x: 0.0, y: 0.0 },
            "b",
        );
        upsert_annotation(db.conn(), &ann_a).expect("upsert a");
        upsert_annotation(db.conn(), &ann_b).expect("upsert b");
        let list_a = list_for_page(db.conn(), page_a, AnnotationFilter::all()).expect("list");
        assert_eq!(list_a.len(), 1);
        assert_eq!(list_a[0].text, "a");
    }
}
