//! Phase 8 Block A — KChat artifact publishing integration tests.
//!
//! Drives the real [`KChatBackendClient::publish_artifact`] +
//! [`KChatBackendClient::list_artifacts`] against the in-process
//! axum fixture. Covers the happy path round-trip + every typed
//! error path the renderer needs to distinguish:
//!
//! - `401` → token refresh + retry (inherited from `request_authed_multipart`)
//! - `415 UNSUPPORTED_ARTIFACT_KIND` → [`ClientError::ArtifactKindUnsupported`]
//! - `413 ARTIFACT_TOO_LARGE` → [`ClientError::ArtifactTooLarge`]
//! - `429` → bounded retry then success
//! - client-side cap at `MAX_ARTIFACT_BYTES` before issuing the request

use kcreate_kchat_client::fixture::{FixtureBehavior, FixtureServer};
use kcreate_kchat_client::{
    ArtifactKind, ArtifactMetadata, ArtifactPublishParams, ArtifactPublishThumbnail, ClientError,
    KChatBackendClient, LoginRequest, MAX_ARTIFACT_BYTES,
};
use uuid::Uuid;

fn login_body(server: &FixtureServer) -> LoginRequest {
    LoginRequest {
        login_id: server.login_id.clone(),
        password: server.password.clone(),
        totp: None,
    }
}

/// PNG magic + a tiny synthetic chunk. The fixture echoes the
/// byte size back so we can assert on a non-zero value without
/// caring whether the bytes are a real valid PNG.
fn png_fixture_bytes() -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    bytes.extend_from_slice(b"trailing fake chunk data");
    bytes
}

fn metadata_fixture() -> ArtifactMetadata {
    ArtifactMetadata {
        project_name: "Hero Banner".into(),
        artboard_name: Some("Page 1".into()),
        export_preset: Some("PNG @1x".into()),
        width_px: Some(1024),
        height_px: Some(1024),
        project_id: Uuid::new_v4(),
        kind: ArtifactKind::Png,
    }
}

fn params_fixture(conversation_id: &str, with_thumb: bool) -> ArtifactPublishParams {
    let png = png_fixture_bytes();
    let thumb = if with_thumb {
        Some(ArtifactPublishThumbnail::from_png(png.clone(), 64, 64).expect("thumbnail magic"))
    } else {
        None
    };
    ArtifactPublishParams {
        conversation_id: conversation_id.into(),
        artifact_bytes: png,
        kind: ArtifactKind::Png,
        thumbnail: thumb,
        metadata: metadata_fixture(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_artifact_round_trips_through_fixture() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");

    let result = client
        .publish_artifact(params_fixture("conv-general", true))
        .await
        .expect("publish");
    assert!(result.artifact_id.starts_with("art-"));
    assert_eq!(result.conversation_id, "conv-general");
    assert_eq!(result.kind, ArtifactKind::Png);
    assert!(result.preview_url.starts_with("https://"));
    assert!(result.thumbnail_url.starts_with("https://"));
    assert_ne!(
        result.preview_url, result.thumbnail_url,
        "thumbnail URL must differ when a thumb is provided"
    );

    // GET path now returns the published artifact.
    let listed = client.list_artifacts("conv-general").await.expect("list");
    assert_eq!(listed.len(), 1);
    let entry = &listed[0];
    assert_eq!(entry.artifact_id, result.artifact_id);
    assert_eq!(entry.byte_size, png_fixture_bytes().len() as u64);
    assert_eq!(entry.metadata.project_name, "Hero Banner");
    assert_eq!(entry.metadata.export_preset.as_deref(), Some("PNG @1x"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_artifact_without_thumbnail_uses_preview_for_thumb_url() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");

    let result = client
        .publish_artifact(params_fixture("conv-general", false))
        .await
        .expect("publish");
    assert_eq!(
        result.preview_url, result.thumbnail_url,
        "thumbnail URL should fall back to preview when no thumb sent"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_artifact_rejects_unsupported_kind() {
    let mut behavior = FixtureBehavior::happy();
    behavior.artifact_kind_rejected = true;
    let server = FixtureServer::spawn(behavior).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");

    let err = client
        .publish_artifact(params_fixture("conv-general", true))
        .await
        .expect_err("must reject");
    assert!(
        matches!(err, ClientError::ArtifactKindUnsupported { .. }),
        "got {err:?}",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_artifact_rejects_oversize_server_side() {
    let mut behavior = FixtureBehavior::happy();
    behavior.artifact_too_large = true;
    let server = FixtureServer::spawn(behavior).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");

    let err = client
        .publish_artifact(params_fixture("conv-general", true))
        .await
        .expect_err("must reject");
    assert!(
        matches!(err, ClientError::ArtifactTooLarge { .. }),
        "got {err:?}",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_artifact_fails_fast_on_oversize_client_side() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");

    let mut params = params_fixture("conv-general", false);
    // Inflate the artifact above the client-side cap. Use a Vec
    // filled with PNG magic bytes — the fixture doesn't validate
    // content beyond the byte size.
    params.artifact_bytes = vec![0u8; MAX_ARTIFACT_BYTES + 1];
    let err = client
        .publish_artifact(params)
        .await
        .expect_err("cap fires");
    assert!(
        matches!(err, ClientError::ArtifactTooLarge { .. }),
        "got {err:?}",
    );
    // No artifact should have hit the fixture.
    let listed = client.list_artifacts("conv-general").await.expect("list");
    assert!(listed.is_empty(), "client cap must short-circuit upload");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_artifact_rejects_empty_conversation_id() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");

    let mut params = params_fixture("conv-general", false);
    params.conversation_id = String::new();
    let err = client.publish_artifact(params).await.expect_err("empty id");
    match err {
        ClientError::Backend { body, .. } => {
            assert_eq!(body.code, "INVALID_REQUEST");
        }
        other => panic!("expected Backend INVALID_REQUEST, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_artifact_rejects_empty_bytes() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");

    let mut params = params_fixture("conv-general", false);
    params.artifact_bytes = Vec::new();
    let err = client
        .publish_artifact(params)
        .await
        .expect_err("empty bytes");
    match err {
        ClientError::Backend { body, .. } => {
            assert_eq!(body.code, "INVALID_REQUEST");
        }
        other => panic!("expected Backend INVALID_REQUEST, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_artifact_retries_on_rate_limit_then_succeeds() {
    let mut behavior = FixtureBehavior::happy();
    behavior.rate_limit_initial = Some(2);
    let server = FixtureServer::spawn(behavior).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    // Login itself burns the rate-limit budget, so prime a fresh
    // behavior set after authentication.
    client.login(&login_body(&server)).await.expect("login");
    // After login the budget is spent; flip to fresh rate-limit
    // budget for the publish call.
    server.set_behavior({
        let mut b = FixtureBehavior::happy();
        b.rate_limit_initial = Some(2);
        b
    });

    let result = client
        .publish_artifact(params_fixture("conv-general", false))
        .await
        .expect("publish after retry");
    assert!(result.artifact_id.starts_with("art-"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_artifact_refreshes_and_retries_on_expired_access_token() {
    let mut behavior = FixtureBehavior::happy();
    behavior.access_token_lifetime_secs = 1;
    let server = FixtureServer::spawn(behavior).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");
    // Wait past the 1s access-token lifetime so the first
    // publish attempt hits 401 → refresh → retry.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let result = client
        .publish_artifact(params_fixture("conv-general", true))
        .await
        .expect("publish after refresh");
    assert!(result.artifact_id.starts_with("art-"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_artifacts_for_unknown_conversation_returns_empty() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");

    let listed = client.list_artifacts("conv-never").await.expect("list");
    assert!(listed.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_artifacts_returns_each_published_in_order() {
    let server = FixtureServer::spawn(FixtureBehavior::happy()).await;
    let client = KChatBackendClient::new_for_tests(&server.base_url).expect("client");
    client.login(&login_body(&server)).await.expect("login");

    for i in 0..3 {
        let mut params = params_fixture("conv-general", false);
        params.metadata.project_name = format!("Project {i}");
        client.publish_artifact(params).await.expect("publish");
    }
    let listed = client.list_artifacts("conv-general").await.expect("list");
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[0].metadata.project_name, "Project 0");
    assert_eq!(listed[1].metadata.project_name, "Project 1");
    assert_eq!(listed[2].metadata.project_name, "Project 2");
}
