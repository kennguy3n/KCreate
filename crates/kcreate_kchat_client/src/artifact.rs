//! Phase 8 Block A — KChat artifact publishing.
//!
//! Adds a multipart-upload pipeline on top of [`KChatBackendClient`]
//! so the renderer can post an exported PNG / SVG / PDF / WebP /
//! JPEG / `.kbrand` archive into a KChat conversation as a rich
//! preview card. The full end-to-end flow is owned by the bridge
//! (`kcreate_bridge::kchat_backend`) which runs the export (or
//! brand-kit serialisation), generates a thumbnail, and hands the
//! resulting bytes to [`KChatBackendClient::publish_artifact`].
//!
//! ## Wire shape
//!
//! `POST /api/v1/conversations/{id}/artifacts` is a
//! `multipart/form-data` request with three named parts:
//!
//! 1. `artifact` — the raw exported bytes, Content-Type set to the
//!    matching MIME (`image/png`, `application/pdf`,
//!    `application/zip`, ...). File name carries the artboard /
//!    project name + extension so the backend can surface a
//!    sensible "download as" filename.
//! 2. `thumbnail` — a PNG-encoded preview the backend should serve
//!    in the rich-card. Always PNG to keep the wire format
//!    predictable. May be omitted by passing `None`; the backend
//!    will fall back to its own renderer if it knows the artifact
//!    type, otherwise show a generic icon.
//! 3. `metadata` — JSON-encoded [`ArtifactMetadata`].
//!
//! Every byte buffer in this module is owned (`Vec<u8>`) so the
//! `request_authed_multipart` builder closure can produce a fresh
//! `multipart::Form` on every retry without lifetime gymnastics.
//!
//! ## Retry semantics
//!
//! Auth + 429 retries are inherited from
//! [`crate::rest::RestClient::request_authed_multipart`]. The
//! artifact-specific status codes (`415 Unsupported Media Type`,
//! `413 Payload Too Large`) are mapped to typed
//! [`ClientError::ArtifactKindUnsupported`] /
//! [`ClientError::ArtifactTooLarge`] inside `rest.rs::classify_failure`
//! so the renderer can show a clean error message instead of a
//! raw status code.
//!
//! ## Size cap
//!
//! Backends commonly cap multipart uploads at 50–100 MB. We
//! perform a *client-side* fail-fast at [`MAX_ARTIFACT_BYTES`]
//! (50 MB) so a runaway 8K PDF export doesn't traverse the wire
//! only to be rejected.

use reqwest::Method;

use crate::client::{urlencoding, KChatBackendClient};
use crate::error::ClientError;
use crate::protocol::{
    artifact_field, ArtifactKind, ArtifactMetadata, ArtifactPublishResult, ArtifactsListResponse,
    PublishedArtifact,
};

/// Hard client-side cap on the artifact byte size we are willing
/// to ship. Mirrors the backend's published per-upload limit
/// (50 MB) so the client surfaces an
/// [`ClientError::ArtifactTooLarge`] *before* a 50 MB body has to
/// stream through the user's connection. Brand kits and PDFs are
/// almost always well under this; raster exports at extreme
/// scale factors can blow past it, in which case the renderer
/// should suggest a lower-scale preset.
pub const MAX_ARTIFACT_BYTES: usize = 50 * 1024 * 1024;

/// PNG-encoded thumbnail accompanying the artifact upload. Always
/// PNG so the rich-card preview pipeline can keep one decode path.
/// Width / height are advisory — backend authors may resize before
/// storing — but they're carried on the wire so the renderer can
/// position the placeholder before the upload completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactPublishThumbnail {
    /// PNG-encoded bytes. Must start with the PNG magic
    /// (`89 50 4E 47 0D 0A 1A 0A`); the backend will reject
    /// anything else with `400 INVALID_REQUEST`.
    pub png_bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl ArtifactPublishThumbnail {
    /// Build a thumbnail wrapper from already-encoded PNG bytes.
    /// The PNG magic is verified eagerly so callers can't smuggle
    /// JPEG / WebP under the `thumbnail` part name.
    pub fn from_png(png_bytes: Vec<u8>, width: u32, height: u32) -> Result<Self, ClientError> {
        const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        if png_bytes.len() < PNG_MAGIC.len() || &png_bytes[..PNG_MAGIC.len()] != PNG_MAGIC {
            return Err(ClientError::Backend {
                status: 0,
                body: crate::protocol::BackendErrorBody {
                    code: "INVALID_THUMBNAIL".into(),
                    message: "thumbnail bytes are not PNG-encoded".into(),
                    data: None,
                },
            });
        }
        Ok(Self {
            png_bytes,
            width,
            height,
        })
    }
}

/// Parameters for [`KChatBackendClient::publish_artifact`]. Pulled
/// out as a struct (vs. positional args) because the renderer-side
/// caller already builds these fields incrementally during the
/// export + thumbnail stages, so a builder-style struct keeps the
/// bridge call site readable.
#[derive(Debug, Clone)]
pub struct ArtifactPublishParams {
    /// Conversation id to post the artifact into. URL-encoded by
    /// the client; callers pass the raw id from
    /// [`crate::client::KChatBackendClient::list_conversations`].
    pub conversation_id: String,
    /// Raw exported bytes (PNG / SVG / PDF / WebP / JPEG / kbrand).
    pub artifact_bytes: Vec<u8>,
    /// Wire format of [`Self::artifact_bytes`].
    pub kind: ArtifactKind,
    /// Optional PNG-encoded preview. When `None`, the backend
    /// falls back to either its own rasteriser (when it understands
    /// the artifact type) or a generic icon.
    pub thumbnail: Option<ArtifactPublishThumbnail>,
    /// Structured metadata serialised into the `metadata` part.
    pub metadata: ArtifactMetadata,
}

impl ArtifactPublishParams {
    /// File-name stem the multipart `artifact` part advertises.
    /// Combines the project name with the optional artboard / page
    /// name and the artifact extension. Sanitised so the backend
    /// can persist the value to disk without escaping. Pure helper
    /// — exposed `pub(crate)` so the unit tests can pin the format.
    pub(crate) fn artifact_filename(&self) -> String {
        let stem = match &self.metadata.artboard_name {
            Some(name) if !name.is_empty() => {
                format!("{}-{}", self.metadata.project_name, name)
            }
            _ => self.metadata.project_name.clone(),
        };
        let safe = sanitize_filename(&stem);
        format!("{}.{}", safe, self.kind.extension())
    }

    /// Default thumbnail filename. PNG always.
    pub(crate) fn thumbnail_filename(&self) -> String {
        let stem = sanitize_filename(&self.metadata.project_name);
        format!("{stem}-thumb.png")
    }
}

/// Replace every byte outside `[A-Za-z0-9._-]` with `_`. The
/// backend treats the filename as opaque, but a sanitised value
/// keeps server-side logs readable and prevents path-traversal
/// from a malicious project name (`../etc/passwd`).
fn sanitize_filename(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("artifact");
    }
    out
}

impl KChatBackendClient {
    /// `POST /api/v1/conversations/{id}/artifacts` — upload an
    /// exported design artifact + optional thumbnail + metadata as
    /// a multipart form. Returns the backend's assigned artifact
    /// id + preview URLs.
    ///
    /// Fail-fast cases:
    /// - empty `artifact_bytes` → `ClientError::Backend` with
    ///   `INVALID_REQUEST` so the caller doesn't waste a round
    ///   trip.
    /// - `artifact_bytes.len() > MAX_ARTIFACT_BYTES` →
    ///   `ClientError::ArtifactTooLarge`.
    /// - empty `conversation_id` → `ClientError::Backend` with
    ///   `INVALID_REQUEST` (the backend would otherwise route the
    ///   request to `/api/v1/conversations//artifacts`, which
    ///   reqwest normalises away).
    pub async fn publish_artifact(
        &self,
        params: ArtifactPublishParams,
    ) -> Result<ArtifactPublishResult, ClientError> {
        if params.conversation_id.is_empty() {
            return Err(ClientError::Backend {
                status: 0,
                body: crate::protocol::BackendErrorBody {
                    code: crate::protocol::error_code::INVALID_REQUEST.into(),
                    message: "conversation_id is empty".into(),
                    data: None,
                },
            });
        }
        if params.artifact_bytes.is_empty() {
            return Err(ClientError::Backend {
                status: 0,
                body: crate::protocol::BackendErrorBody {
                    code: crate::protocol::error_code::INVALID_REQUEST.into(),
                    message: "artifact_bytes is empty".into(),
                    data: None,
                },
            });
        }
        if params.artifact_bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(ClientError::ArtifactTooLarge {
                message: format!(
                    "artifact is {} bytes; client-side cap is {}",
                    params.artifact_bytes.len(),
                    MAX_ARTIFACT_BYTES
                ),
            });
        }
        // Serialise metadata eagerly so we surface a malformed
        // metadata struct (e.g. UTF-8 from a buggy project name)
        // before issuing the request.
        let metadata_json =
            serde_json::to_vec(&params.metadata).map_err(|e| ClientError::Deserialization {
                path: "/api/v1/conversations/{id}/artifacts (metadata serialize)".into(),
                message: e.to_string(),
            })?;

        let path = format!(
            "/api/v1/conversations/{}/artifacts",
            urlencoding(&params.conversation_id),
        );

        // The form builder is invoked on every retry — that's the
        // whole reason `request_authed_multipart` takes a closure.
        // We move owned `Vec<u8>` slices into the closure so each
        // retry gets fresh ownership without copying.
        let artifact_filename = params.artifact_filename();
        let thumbnail_filename = params.thumbnail_filename();
        let artifact_mime = params.kind.mime();
        let kind = params.kind;

        let build_form = move || {
            let artifact_part = reqwest::multipart::Part::bytes(params.artifact_bytes.clone())
                .file_name(artifact_filename.clone())
                .mime_str(artifact_mime)
                .expect("static MIME literal parses");
            let mut form = reqwest::multipart::Form::new()
                .part(artifact_field::ARTIFACT, artifact_part)
                .part(
                    artifact_field::METADATA,
                    reqwest::multipart::Part::bytes(metadata_json.clone())
                        .file_name("metadata.json")
                        .mime_str("application/json")
                        .expect("static MIME literal parses"),
                );
            if let Some(thumb) = params.thumbnail.clone() {
                let thumb_part = reqwest::multipart::Part::bytes(thumb.png_bytes)
                    .file_name(thumbnail_filename.clone())
                    .mime_str("image/png")
                    .expect("static MIME literal parses");
                form = form.part(artifact_field::THUMBNAIL, thumb_part);
            }
            // Tag the kind on a string field so the backend can
            // route the upload to the right preview pipeline even
            // before sniffing the artifact bytes.
            form.text("kind", kind_text(kind))
        };

        self.rest()
            .request_authed_multipart::<ArtifactPublishResult, _>(Method::POST, &path, build_form)
            .await
    }

    /// `GET /api/v1/conversations/{id}/artifacts` — list artifacts
    /// previously published to the conversation. Useful for the
    /// renderer's "recent artifacts" pane and for integration
    /// tests asserting the publish round-trip.
    pub async fn list_artifacts(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<PublishedArtifact>, ClientError> {
        if conversation_id.is_empty() {
            return Err(ClientError::Backend {
                status: 0,
                body: crate::protocol::BackendErrorBody {
                    code: crate::protocol::error_code::INVALID_REQUEST.into(),
                    message: "conversation_id is empty".into(),
                    data: None,
                },
            });
        }
        let path = format!(
            "/api/v1/conversations/{}/artifacts",
            urlencoding(conversation_id),
        );
        let resp: ArtifactsListResponse = self
            .rest()
            .request_authed::<(), _>(Method::GET, &path, None)
            .await?;
        Ok(resp.artifacts)
    }
}

/// Lower-case text form of [`ArtifactKind`], used by the `kind`
/// form field. Mirrors the serde representation so backend
/// implementations can either parse the string or inspect the
/// Content-Type — the values must agree.
const fn kind_text(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Png => "png",
        ArtifactKind::Svg => "svg",
        ArtifactKind::Pdf => "pdf",
        ArtifactKind::Webp => "webp",
        ArtifactKind::Jpeg => "jpeg",
        ArtifactKind::BrandKit => "brandkit",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn metadata_fixture() -> ArtifactMetadata {
        ArtifactMetadata {
            project_name: "Hero Banner".into(),
            artboard_name: Some("Page 1".into()),
            export_preset: Some("PNG @1x".into()),
            width_px: Some(1024),
            height_px: Some(1024),
            project_id: Uuid::nil(),
            kind: ArtifactKind::Png,
        }
    }

    #[test]
    fn sanitize_filename_passes_safe_ascii() {
        assert_eq!(sanitize_filename("Hero-Banner_v1.0"), "Hero-Banner_v1.0");
    }

    #[test]
    fn sanitize_filename_strips_traversal() {
        assert_eq!(sanitize_filename("../etc/passwd"), ".._etc_passwd");
    }

    #[test]
    fn sanitize_filename_falls_back_when_empty() {
        // All-spaces collapses to underscores; non-empty after
        // sanitisation so the fallback shouldn't fire.
        assert_eq!(sanitize_filename("   "), "___");
        // Pure unicode reduces to underscores (still non-empty).
        assert_eq!(sanitize_filename("✨🚀"), "__");
        // Truly empty input does fall back.
        assert_eq!(sanitize_filename(""), "artifact");
    }

    #[test]
    fn artifact_filename_combines_project_and_artboard() {
        let p = ArtifactPublishParams {
            conversation_id: "conv-1".into(),
            artifact_bytes: vec![0u8; 4],
            kind: ArtifactKind::Png,
            thumbnail: None,
            metadata: metadata_fixture(),
        };
        assert_eq!(p.artifact_filename(), "Hero_Banner-Page_1.png");
        assert_eq!(p.thumbnail_filename(), "Hero_Banner-thumb.png");
    }

    #[test]
    fn artifact_filename_uses_project_when_no_artboard() {
        let mut meta = metadata_fixture();
        meta.artboard_name = None;
        let p = ArtifactPublishParams {
            conversation_id: "conv-1".into(),
            artifact_bytes: vec![0u8; 4],
            kind: ArtifactKind::Pdf,
            thumbnail: None,
            metadata: meta,
        };
        assert_eq!(p.artifact_filename(), "Hero_Banner.pdf");
    }

    #[test]
    fn thumbnail_from_png_rejects_non_png() {
        // SVG bytes — not PNG. Must fail.
        let r = ArtifactPublishThumbnail::from_png(b"<svg/>".to_vec(), 16, 16);
        assert!(matches!(r, Err(ClientError::Backend { .. })));
    }

    #[test]
    fn thumbnail_from_png_accepts_png_magic() {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend_from_slice(b"trailing chunk data");
        let t = ArtifactPublishThumbnail::from_png(bytes, 64, 64).expect("png magic accepted");
        assert_eq!(t.width, 64);
        assert_eq!(t.height, 64);
    }

    #[test]
    fn artifact_kind_mime_and_extension_are_aligned() {
        // Spot check every variant so a future addition doesn't
        // forget to update both halves of the mime/ext mapping.
        for (kind, mime, ext) in [
            (ArtifactKind::Png, "image/png", "png"),
            (ArtifactKind::Svg, "image/svg+xml", "svg"),
            (ArtifactKind::Pdf, "application/pdf", "pdf"),
            (ArtifactKind::Webp, "image/webp", "webp"),
            (ArtifactKind::Jpeg, "image/jpeg", "jpeg"),
            (ArtifactKind::BrandKit, "application/zip", "kbrand"),
        ] {
            assert_eq!(kind.mime(), mime, "{kind:?}");
            assert_eq!(kind.extension(), ext, "{kind:?}");
        }
    }
}
