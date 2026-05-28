//! Phase 8 Block A — KChat artifact-publish bridge integration test.
//!
//! Drives the bridge entry points
//! [`kchat_backend::kchat_backend_connect_for_tests`],
//! [`kchat_artifact::kchat_backend_publish_artifact`], and
//! [`kchat_artifact::kchat_backend_list_artifacts`] end-to-end
//! against the same in-process `axum` fixture
//! `crates/kcreate_kchat_client/tests/artifact_round_trip.rs`
//! uses. This is the "bridge half" — the client half is already
//! exhaustively tested at the `kcreate_kchat_client` crate level.
//!
//! What this test pins:
//!
//! 1. The wire-format `KChatArtifactRequest` JSON the renderer
//!    sends through `kchat_backend_publish_artifact` deserializes
//!    correctly inside the bridge.
//! 2. The bridge stamps `ArtifactMetadata.project_id` /
//!    `project_name` from the live workspace identity.
//! 3. The published artifact round-trips through the
//!    `list_artifacts` GET so the renderer's "recent artifacts"
//!    pane will see what it just published.
//! 4. The `NoOpenProject` / `NotConnected` error paths surface as
//!    typed errors rather than silently producing a bogus result.
//!
//! `serial_test` is required because the bridge's KChat client
//! slot AND the document workspace are both process-global
//! singletons.

// NOTE: `kcreate_tests` enables `kchat-backend` on the `kcreate_bridge`
// dev-dep so this file always links the bridge's KChat surface.
// The local-first deny-list in `local_first.rs` walks the editing-
// path crates with DEFAULT features only — enabling `kchat-backend`
// here as a dev-dep does NOT pull `reqwest`/`tokio` into the
// editing-path tree, so the invariant stays green.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use kcreate_bridge::document::{project_close, project_create, project_save};
use kcreate_bridge::kchat_artifact::{
    kchat_backend_list_artifacts, kchat_backend_publish_artifact, KChatArtifactRequest,
    KChatArtifactRequestKind, KChatSvgArtifactRequest,
};
use kcreate_bridge::kchat_backend::{
    kchat_backend_connect_for_tests, kchat_backend_disconnect, KChatBackendBridgeError,
    KChatBackendSignInRequest,
};
use kcreate_export::svg::SvgExportOptions;
use kcreate_kchat_client::fixture::{FixtureBehavior, FixtureServer};
use kcreate_kchat_client::ArtifactKind;
use serial_test::serial;
use tempfile::TempDir;
use tokio::runtime::Runtime;

/// Dedicated runtime for spawning the axum fixture. We keep it as
/// a `OnceLock` so successive tests reuse the same runtime —
/// spawning a fresh one per test would race the bridge's own
/// global runtime under `cargo test`'s thread pool.
fn fixture_runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("kchat-artifact-int-fixture")
            .build()
            .expect("fixture runtime")
    })
}

/// Hold onto the spawned `FixtureServer` for the lifetime of one
/// test. Dropping it on test exit shuts the axum task down
/// cleanly so the next test gets a fresh port.
struct FixtureGuard {
    inner: Option<FixtureServer>,
}

impl FixtureGuard {
    fn spawn(behavior: FixtureBehavior) -> Self {
        let server = fixture_runtime().block_on(FixtureServer::spawn(behavior));
        Self {
            inner: Some(server),
        }
    }

    fn base_url(&self) -> &str {
        &self.inner.as_ref().expect("fixture alive").base_url
    }

    fn login_id(&self) -> String {
        self.inner.as_ref().expect("fixture alive").login_id.clone()
    }

    fn password(&self) -> String {
        self.inner.as_ref().expect("fixture alive").password.clone()
    }
}

impl Drop for FixtureGuard {
    fn drop(&mut self) {
        // Dropping the inner `FixtureServer` fires the shutdown
        // oneshot; the spawned task will exit on its own thread.
        self.inner.take();
    }
}

/// Single mutex held across each `#[serial]` test for an extra
/// layer of process-global serialisation. `serial_test` already
/// runs tests in this file one at a time, but we also need to
/// serialise against any *other* test in the workspace that
/// touches the document slot or the kchat client slot. In
/// practice the only other consumer is
/// `kchat_backend_client.rs`, which is in this same binary, but
/// we keep the explicit lock so the assertion below ("publish
/// without project errors") doesn't race a later test that
/// happens to open a project.
fn global_lock() -> &'static Mutex<()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
}

fn open_project(name: &str) -> (TempDir, PathBuf) {
    // Defensive close in case a previous test left a workspace
    // installed in the slot. `project_close` is a no-op when the
    // slot is empty so we can call it unconditionally.
    project_close();
    let dir = TempDir::new().expect("tmpdir");
    let path = dir.path().to_path_buf();
    let info = project_create(name, &path).expect("project_create");
    assert_eq!(info.name, name);
    // Persist so anything that revisits the on-disk store after
    // close sees a consistent baseline.
    project_save().expect("project_save");
    (dir, path)
}

fn connect_to_fixture(fixture: &FixtureGuard) {
    let status = kchat_backend_connect_for_tests(KChatBackendSignInRequest {
        base_url: fixture.base_url().to_string(),
        login_id: fixture.login_id(),
        password: fixture.password(),
        totp: None,
    })
    .expect("connect for tests");
    assert!(
        status.connected,
        "connect_for_tests should report connected"
    );
}

fn svg_request() -> KChatArtifactRequest {
    KChatArtifactRequest {
        kind: KChatArtifactRequestKind::Svg(KChatSvgArtifactRequest {
            options: SvgExportOptions::default(),
            node_ids: Vec::new(),
        }),
        export_preset: Some("SVG clean".into()),
        artboard_name: Some("Page 1".into()),
    }
}

#[test]
#[serial]
fn publish_svg_round_trips_through_bridge_and_lists() {
    let _lock = global_lock().lock().expect("global lock");
    let fixture = FixtureGuard::spawn(FixtureBehavior::happy());
    let (_dir, _path) = open_project("Hero Banner");
    connect_to_fixture(&fixture);

    let result =
        kchat_backend_publish_artifact("conv-general", svg_request()).expect("publish svg");
    assert_eq!(result.conversation_id, "conv-general");
    assert_eq!(result.kind, ArtifactKind::Svg);
    assert!(
        result.artifact_id.starts_with("art-"),
        "fixture mints synthetic art-* ids; got {}",
        result.artifact_id,
    );
    assert!(
        result.preview_url.starts_with("https://"),
        "fixture preview URL must be https; got {}",
        result.preview_url,
    );

    // The list endpoint must echo the artifact we just published —
    // proves the upload reached the fixture's in-memory store and
    // the bridge's `list_artifacts` round-trips the metadata.
    let listed = kchat_backend_list_artifacts("conv-general").expect("list artifacts");
    assert_eq!(listed.len(), 1, "exactly one artifact should be present");
    let entry = &listed[0];
    assert_eq!(entry.artifact_id, result.artifact_id);
    assert_eq!(entry.kind, ArtifactKind::Svg);
    assert_eq!(entry.metadata.project_name, "Hero Banner");
    assert_eq!(
        entry.metadata.export_preset.as_deref(),
        Some("SVG clean"),
        "preset must round-trip through the wire-format metadata",
    );
    assert_eq!(
        entry.metadata.artboard_name.as_deref(),
        Some("Page 1"),
        "artboard name must round-trip through the wire-format metadata",
    );

    // Clean up so the next test starts from a known-empty state.
    let _ = kchat_backend_disconnect();
    project_close();
}

#[test]
#[serial]
fn publish_without_project_returns_no_open_project_error() {
    let _lock = global_lock().lock().expect("global lock");
    let fixture = FixtureGuard::spawn(FixtureBehavior::happy());
    // Defensive close: an earlier test may have failed midway and
    // left a workspace installed in the slot. `project_close`
    // tolerates the no-op case.
    project_close();
    connect_to_fixture(&fixture);

    let err = kchat_backend_publish_artifact("conv-general", svg_request())
        .expect_err("must error with no project open");
    assert!(
        matches!(err, KChatBackendBridgeError::NoOpenProject),
        "expected NoOpenProject, got {err:?}",
    );

    let _ = kchat_backend_disconnect();
}

#[test]
#[serial]
fn publish_without_client_returns_not_connected_error() {
    let _lock = global_lock().lock().expect("global lock");
    // No fixture spawn + no `connect_for_tests`: the client slot
    // is empty, so any attempt to publish must surface
    // `NotConnected` BEFORE touching the renderer / workspace.
    let _ = kchat_backend_disconnect();
    let (_dir, _path) = open_project("Disconnected");

    let err = kchat_backend_publish_artifact("conv-general", svg_request())
        .expect_err("must error with no client installed");
    assert!(
        matches!(err, KChatBackendBridgeError::NotConnected),
        "expected NotConnected, got {err:?}",
    );

    project_close();
}

#[test]
#[serial]
fn publish_with_empty_conversation_id_rejects_invalid_arg() {
    let _lock = global_lock().lock().expect("global lock");
    let fixture = FixtureGuard::spawn(FixtureBehavior::happy());
    let (_dir, _path) = open_project("Empty Conv");
    connect_to_fixture(&fixture);

    let err = kchat_backend_publish_artifact("", svg_request())
        .expect_err("empty conversation id must be rejected client-side");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("conversation_id"),
        "error should call out the empty conversation id, got {msg}",
    );

    let _ = kchat_backend_disconnect();
    project_close();
}
