//! Project-lifecycle integration test.
//!
//! Exercises the full local-first round trip across `kcreate_core` and
//! `kcreate_storage`: create a project on disk, mutate the document
//! graph, push operations, save to `SQLite`, close, reopen, and verify
//! every node + operation is recoverable.

use kcreate_core::node::{Bounds, Node, NodeType, Transform2D};
use kcreate_core::operation::Operation;
use kcreate_storage::ProjectStore;
use tempfile::TempDir;

#[test]
fn create_add_save_reopen_round_trip() {
    let dir = TempDir::new().expect("tempdir");
    let project_dir = dir.path().join("acme.kstudio");

    // 1. Create project on disk.
    let mut store = ProjectStore::create(&project_dir, "acme").expect("create");
    assert!(project_dir.is_dir(), ".kstudio directory must exist");
    assert!(
        project_dir.join("manifest.json").is_file(),
        "manifest must be written eagerly",
    );

    // 2. Build a small document graph: one page, one artboard, one
    //    vector layer.
    let mut graph = kcreate_core::document::DocumentGraph::new();
    let page_id = graph
        .insert_node(Node::new(NodeType::Page, "Page 1"))
        .expect("page");
    let mut artboard = Node::new(NodeType::Artboard, "Artboard 1");
    artboard.parent_id = Some(page_id);
    artboard.bounds = Bounds::new(0.0, 0.0, 800.0, 600.0);
    artboard.transform = Transform2D::IDENTITY;
    let artboard_id = graph.insert_node(artboard).expect("artboard");
    let mut vector = Node::new(NodeType::VectorLayer, "Hero shape");
    vector.parent_id = Some(artboard_id);
    let vector_id = graph.insert_node(vector).expect("vector");

    // 3. Persist the document.
    store.save_document(&graph).expect("save_document");

    // 4. Record an operation; it must survive close/reopen too.
    let op = Operation::new(
        "user",
        "node.create",
        serde_json::Value::Null,
        serde_json::json!({ "id": vector_id }),
        vec![vector_id],
    );
    let op_id = op.id;
    store.save_operation(&op).expect("save_op");

    // 5. Close (drop the store) and reopen from disk.
    drop(store);
    let store2 = ProjectStore::open(&project_dir).expect("reopen");

    // 6. The manifest is still readable and points at the same project.
    let manifest = store2.manifest();
    assert_eq!(manifest.name, "acme");

    // 7. Document graph round-trips with identical node ids.
    let graph2 = store2.load_document().expect("load_document");
    assert!(graph2.get_node(page_id).is_some(), "page must be persisted");
    assert!(
        graph2.get_node(artboard_id).is_some(),
        "artboard must be persisted",
    );
    assert!(
        graph2.get_node(vector_id).is_some(),
        "vector layer must be persisted",
    );
    assert_eq!(graph2.node_count(), graph.node_count());

    // 8. Operations are also durable.
    let ops = store2.load_operations(100).expect("load_ops");
    assert!(
        ops.iter().any(|o| o.id == op_id),
        "saved operation must be recoverable: {ops:?}",
    );
}

#[test]
fn store_asset_is_deduplicated_by_content_hash() {
    let dir = TempDir::new().expect("tempdir");
    let project_dir = dir.path().join("media.kstudio");
    let mut store = ProjectStore::create(&project_dir, "media").expect("create");

    // The same bytes must always hash to the same content path —
    // BLAKE3 is the contract.
    let payload = b"the quick brown fox jumps over the lazy dog";
    let a = store.store_asset(payload, "text/plain").expect("store a");
    let b = store.store_asset(payload, "text/plain").expect("store b");
    assert_eq!(a.hash, b.hash, "identical bytes share a hash");
    assert_eq!(a.path, b.path, "and a path");
}
