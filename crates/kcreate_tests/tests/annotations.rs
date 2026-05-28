//! Phase 8 Block A: design review annotations.
//!
//! Storage-level CRUD round-trip between
//! [`kcreate_core::Annotation`] and the SQLite-backed store in
//! [`kcreate_storage::annotations`] is verified first. Bridge-level
//! CRUD (the `annotation_create` / `annotation_reply` /
//! `annotation_list` / `annotation_resolve` / `annotation_delete`
//! entry points exposed over the N-API surface) is verified second.
//! Network broadcast of annotations is verified separately in
//! `collab_realtime.rs`; this file pins the local-first half.

use chrono::Utc;
use kcreate_core::{Annotation, AnnotationFilter, AnnotationPosition};
use kcreate_storage::annotations::{
    delete_annotation, list_all, list_for_page, load_annotation, set_resolved, upsert_annotation,
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

#[test]
fn load_annotation_returns_none_for_unknown_id() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("annotations.db")).unwrap();
    let conn = db.conn();
    assert!(load_annotation(conn, Uuid::new_v4()).unwrap().is_none());
}

// --- Bridge-level CRUD tests --------------------------------------------
//
// These exercise `annotation_create` / `annotation_reply` /
// `annotation_list` / `annotation_resolve` / `annotation_delete` against
// a real `Workspace` opened through `project_create`. The bridge code
// path is what the renderer actually invokes via N-API, so pinning it
// catches regressions in the workspace-mutex + storage glue that the
// pure-storage tests above can't reach. `serial_test` is required
// because the workspace slot is a process-global singleton.

mod bridge_integration {
    use kcreate_bridge::annotation_bridge::{
        annotation_create, annotation_delete, annotation_list, annotation_reply,
        annotation_resolve, AnnotationCreateRequest, AnnotationListRequest, AnnotationReplyRequest,
        AnnotationResolveRequest,
    };
    use kcreate_bridge::document::{document_get_tree, project_close, project_create};
    use kcreate_core::AnnotationPosition;
    use serial_test::serial;
    use uuid::Uuid;

    /// Open a fresh project under a temp dir, returning the id of
    /// the first page that gets auto-created by `project_create`.
    /// Callers must let the returned `TempDir` outlive any
    /// workspace use so the on-disk SQLite stays alive.
    fn open_with_page(name: &str) -> (tempfile::TempDir, Uuid) {
        project_close();
        let dir = tempfile::tempdir().expect("tmpdir");
        project_create(name, dir.path()).expect("project_create");
        let tree = document_get_tree().expect("tree");
        let page_id = tree.first().expect("page present").id;
        (dir, page_id)
    }

    fn create_request(page_id: Uuid, text: &str) -> AnnotationCreateRequest {
        AnnotationCreateRequest {
            page_id,
            author_peer_id: "peer-bridge".to_string(),
            author_name: "Bridge Tester".to_string(),
            position: AnnotationPosition { x: 42.0, y: 17.0 },
            text: text.to_string(),
        }
    }

    fn list_all_request(page_id: Uuid) -> AnnotationListRequest {
        AnnotationListRequest {
            page_id,
            include_resolved: true,
            include_unresolved: true,
        }
    }

    #[test]
    #[serial]
    fn create_persists_and_list_returns_it() {
        let (_dir, page_id) = open_with_page("ann-create");
        let ann = annotation_create(create_request(page_id, "Tighten margin")).expect("create");
        assert_eq!(ann.text, "Tighten margin");
        assert!(!ann.resolved, "new annotation should start unresolved");

        let listed = annotation_list(list_all_request(page_id)).expect("list");
        assert_eq!(listed.annotations.len(), 1);
        assert_eq!(listed.annotations[0].id, ann.id);
        project_close();
    }

    #[test]
    #[serial]
    fn list_filter_hides_resolved_when_disabled() {
        let (_dir, page_id) = open_with_page("ann-filter");
        let open_ann = annotation_create(create_request(page_id, "open")).expect("create open");
        let done_ann = annotation_create(create_request(page_id, "done")).expect("create done");
        annotation_resolve(AnnotationResolveRequest {
            id: done_ann.id,
            resolved: true,
        })
        .expect("resolve");

        let only_open = annotation_list(AnnotationListRequest {
            page_id,
            include_resolved: false,
            include_unresolved: true,
        })
        .expect("list open");
        assert_eq!(only_open.annotations.len(), 1);
        assert_eq!(only_open.annotations[0].id, open_ann.id);

        let only_done = annotation_list(AnnotationListRequest {
            page_id,
            include_resolved: true,
            include_unresolved: false,
        })
        .expect("list done");
        assert_eq!(only_done.annotations.len(), 1);
        assert_eq!(only_done.annotations[0].id, done_ann.id);
        project_close();
    }

    #[test]
    #[serial]
    fn resolve_toggles_state() {
        let (_dir, page_id) = open_with_page("ann-resolve");
        let ann = annotation_create(create_request(page_id, "review")).expect("create");

        let now_resolved = annotation_resolve(AnnotationResolveRequest {
            id: ann.id,
            resolved: true,
        })
        .expect("resolve true");
        assert!(now_resolved);

        let listed = annotation_list(list_all_request(page_id)).expect("list");
        assert!(listed.annotations[0].resolved);

        let now_open = annotation_resolve(AnnotationResolveRequest {
            id: ann.id,
            resolved: false,
        })
        .expect("resolve false");
        assert!(!now_open);

        let listed = annotation_list(list_all_request(page_id)).expect("list 2");
        assert!(!listed.annotations[0].resolved);
        project_close();
    }

    #[test]
    #[serial]
    fn delete_removes_and_second_call_is_noop() {
        let (_dir, page_id) = open_with_page("ann-delete");
        let ann = annotation_create(create_request(page_id, "doomed")).expect("create");
        assert!(annotation_delete(ann.id).expect("delete 1"));
        assert!(!annotation_delete(ann.id).expect("delete 2"));
        let listed = annotation_list(list_all_request(page_id)).expect("list");
        assert!(listed.annotations.is_empty());
        project_close();
    }

    #[test]
    #[serial]
    fn reply_attaches_to_thread_root() {
        let (_dir, page_id) = open_with_page("ann-reply");
        let head = annotation_create(create_request(page_id, "head")).expect("create head");
        let reply = annotation_reply(AnnotationReplyRequest {
            parent_id: head.id,
            author_peer_id: "peer-b".to_string(),
            author_name: "Beatrice".to_string(),
            text: "Reply".to_string(),
        })
        .expect("reply");
        let nested = annotation_reply(AnnotationReplyRequest {
            parent_id: reply.id,
            author_peer_id: "peer-c".to_string(),
            author_name: "Cory".to_string(),
            text: "Nested".to_string(),
        })
        .expect("nested reply");

        // Both replies attach to the same thread root (the head's
        // id, since the head has no thread_id of its own).
        let root = head.id;
        assert_eq!(reply.thread_id, Some(root));
        assert_eq!(nested.thread_id, Some(root));

        let listed = annotation_list(list_all_request(page_id)).expect("list");
        assert_eq!(listed.annotations.len(), 3);
        project_close();
    }

    #[test]
    #[serial]
    fn reply_to_unknown_parent_errors() {
        let (_dir, _page_id) = open_with_page("ann-reply-err");
        let err = annotation_reply(AnnotationReplyRequest {
            parent_id: Uuid::new_v4(),
            author_peer_id: "peer-x".to_string(),
            author_name: "Phantom".to_string(),
            text: "into the void".to_string(),
        });
        assert!(err.is_err(), "reply to unknown parent must fail");
        project_close();
    }

    #[test]
    #[serial]
    fn resolve_unknown_id_errors() {
        let (_dir, _page_id) = open_with_page("ann-resolve-err");
        let err = annotation_resolve(AnnotationResolveRequest {
            id: Uuid::new_v4(),
            resolved: true,
        });
        assert!(err.is_err(), "resolve unknown id must fail");
        project_close();
    }
}

// --- Collab broadcast wire-format -----------------------------------------
//
// Pins the `Message::AnnotationBroadcast` envelope shape (upsert /
// delete kinds round-trip through the canonical JSON serializer) so
// any future serde rename or variant reshuffle fails fast at
// compile/test time instead of silently breaking peer convergence.

#[test]
fn annotation_broadcast_upsert_envelope_round_trips() {
    use kcreate_collab::{AnnotationBroadcastKind, AnnotationBroadcastPayload, Message};

    let ann = make_annotation(Uuid::new_v4(), "broadcast me");
    let payload = AnnotationBroadcastPayload {
        project_id: Uuid::new_v4(),
        kind: AnnotationBroadcastKind::Upsert,
        annotations: vec![ann.clone()],
        sent_at: Utc::now(),
    };
    let msg = Message::AnnotationBroadcast(payload);
    let json = serde_json::to_string(&msg).expect("serialize");
    assert!(
        json.contains("\"kind\":\"annotation_broadcast\""),
        "must use external tag annotation_broadcast: {json}"
    );
    assert!(
        json.contains("\"kind\":\"upsert\""),
        "kind discriminator must be snake_case: {json}"
    );
    let back: Message = serde_json::from_str(&json).expect("deserialize");
    match back {
        Message::AnnotationBroadcast(p) => {
            assert_eq!(p.kind, AnnotationBroadcastKind::Upsert);
            assert_eq!(p.annotations.len(), 1);
            assert_eq!(p.annotations[0].id, ann.id);
        }
        other => panic!("expected AnnotationBroadcast, got {other:?}"),
    }
}

#[test]
fn annotation_broadcast_delete_envelope_round_trips() {
    use kcreate_collab::{AnnotationBroadcastKind, AnnotationBroadcastPayload, Message};

    let ann = make_annotation(Uuid::new_v4(), "doomed");
    let payload = AnnotationBroadcastPayload {
        project_id: Uuid::new_v4(),
        kind: AnnotationBroadcastKind::Delete,
        annotations: vec![ann],
        sent_at: Utc::now(),
    };
    let msg = Message::AnnotationBroadcast(payload);
    let json = serde_json::to_string(&msg).expect("serialize");
    assert!(
        json.contains("\"kind\":\"delete\""),
        "delete kind must round-trip: {json}"
    );
    let back: Message = serde_json::from_str(&json).expect("deserialize");
    assert!(
        matches!(back, Message::AnnotationBroadcast(p) if p.kind == AnnotationBroadcastKind::Delete)
    );
}
