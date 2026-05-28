//! Phase 8 Block A: design review annotations.
//!
//! Tests the cross-crate CRUD round-trip between
//! [`kcreate_core::Annotation`] and the SQLite-backed store in
//! [`kcreate_storage::annotations`]. Network broadcast of
//! annotations is verified separately in `collab_realtime.rs` once
//! the bridge surface lands; this file focuses on the storage +
//! filter contract.

use chrono::Utc;
use kcreate_core::{Annotation, AnnotationFilter, AnnotationPosition};
use kcreate_storage::annotations::{
    delete_annotation, list_all, list_for_page, set_resolved, upsert_annotation,
};
use kcreate_storage::Database;
use uuid::Uuid;

fn make_annotation(page: Uuid, text: &str) -> Annotation {
    Annotation {
        id: Uuid::new_v4(),
        page_id: page,
        author_peer_id: "peer-a".into(),
        author_name: "Ada".into(),
        position: AnnotationPosition { x: 10.0, y: 20.0 },
        text: text.into(),
        timestamp: Utc::now(),
        resolved: false,
        thread_id: None,
    }
}

#[test]
fn crud_round_trip_persists_and_retrieves() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("annotations.db")).unwrap();
    let conn = db.conn();
    let page = Uuid::new_v4();
    let ann = make_annotation(page, "Tighten this margin");
    upsert_annotation(conn, &ann).unwrap();
    let listed = list_for_page(conn, page, AnnotationFilter::all()).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, ann.id);
    assert_eq!(listed[0].text, "Tighten this margin");
}

#[test]
fn resolve_unresolve_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("annotations.db")).unwrap();
    let conn = db.conn();
    let page = Uuid::new_v4();
    let ann = make_annotation(page, "Needs review");
    upsert_annotation(conn, &ann).unwrap();
    assert_eq!(set_resolved(conn, ann.id, true).unwrap(), Some(true));
    let resolved = list_for_page(conn, page, AnnotationFilter::all()).unwrap();
    assert!(resolved[0].resolved);
    assert_eq!(set_resolved(conn, ann.id, false).unwrap(), Some(false));
    let unresolved = list_for_page(conn, page, AnnotationFilter::all()).unwrap();
    assert!(!unresolved[0].resolved);
}

#[test]
fn filter_unresolved_excludes_resolved() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("annotations.db")).unwrap();
    let conn = db.conn();
    let page = Uuid::new_v4();
    let mut a = make_annotation(page, "a");
    let b = make_annotation(page, "b");
    a.resolved = true;
    upsert_annotation(conn, &a).unwrap();
    upsert_annotation(conn, &b).unwrap();
    let unresolved = list_for_page(conn, page, AnnotationFilter::unresolved_only()).unwrap();
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].id, b.id);
}

#[test]
fn per_page_filtering_isolates_pages() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("annotations.db")).unwrap();
    let conn = db.conn();
    let page_a = Uuid::new_v4();
    let page_b = Uuid::new_v4();
    upsert_annotation(conn, &make_annotation(page_a, "on a")).unwrap();
    upsert_annotation(conn, &make_annotation(page_b, "on b")).unwrap();
    upsert_annotation(conn, &make_annotation(page_b, "on b 2")).unwrap();
    let only_b = list_for_page(conn, page_b, AnnotationFilter::all()).unwrap();
    assert_eq!(only_b.len(), 2);
    let only_a = list_for_page(conn, page_a, AnnotationFilter::all()).unwrap();
    assert_eq!(only_a.len(), 1);
    let all = list_all(conn, AnnotationFilter::all()).unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn delete_removes_annotation() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("annotations.db")).unwrap();
    let conn = db.conn();
    let page = Uuid::new_v4();
    let ann = make_annotation(page, "doomed");
    upsert_annotation(conn, &ann).unwrap();
    assert!(delete_annotation(conn, ann.id).unwrap());
    let empty = list_for_page(conn, page, AnnotationFilter::all()).unwrap();
    assert!(empty.is_empty());
    // Second delete is a no-op.
    assert!(!delete_annotation(conn, ann.id).unwrap());
}

#[test]
fn reply_inherits_thread_root() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("annotations.db")).unwrap();
    let conn = db.conn();
    let page = Uuid::new_v4();
    let parent = make_annotation(page, "Move me");
    let reply = Annotation::reply(&parent, "peer-b", "Beatrice", "Done");
    upsert_annotation(conn, &parent).unwrap();
    upsert_annotation(conn, &reply).unwrap();
    let listed = list_for_page(conn, page, AnnotationFilter::all()).unwrap();
    assert_eq!(listed.len(), 2);
    // Replies inherit the parent's thread root, so both share it.
    let root = listed
        .iter()
        .find(|a| a.id == parent.id)
        .unwrap()
        .thread_id
        .unwrap_or(parent.id);
    for a in &listed {
        let t = a.thread_id.unwrap_or(a.id);
        assert_eq!(t, root);
    }
}

#[test]
fn concurrent_writes_from_two_peers_both_persist() {
    // Simulates the storage half of the collab flow: two peers
    // each upsert one annotation, and both should be present.
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("annotations.db")).unwrap();
    let conn = db.conn();
    let page = Uuid::new_v4();
    let mut from_a = make_annotation(page, "alpha");
    from_a.author_peer_id = "peer-a".into();
    let mut from_b = make_annotation(page, "bravo");
    from_b.author_peer_id = "peer-b".into();
    upsert_annotation(conn, &from_a).unwrap();
    upsert_annotation(conn, &from_b).unwrap();
    let listed = list_for_page(conn, page, AnnotationFilter::all()).unwrap();
    assert_eq!(listed.len(), 2);
    let authors: std::collections::HashSet<_> =
        listed.iter().map(|a| a.author_peer_id.clone()).collect();
    assert!(authors.contains("peer-a"));
    assert!(authors.contains("peer-b"));
}
