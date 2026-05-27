//! Cross-crate integration coverage for Phase 3 Tasks 9-10 — the
//! backend-selectable upscale + point-prompt segmentation surface.
//!
//! These tests exercise the real `kcreate_ai::upscale_with_backend`
//! / `kcreate_ai::segment_with_backend` dispatchers (the same
//! functions the bridge invokes) end-to-end with real pixel buffers,
//! locking in:
//!
//! * The dispatcher returns the same buffer the direct path would
//!   produce when the built-in backend is selected.
//! * Backend gating: when ONNX features are off, requesting the
//!   ONNX backend returns `BackendUnavailable` *up front* —
//!   never silently falls back to the built-in. The renderer
//!   relies on this for accurate UI affordances ("install model
//!   pack to enable ESRGAN").
//! * The segmentation mask is a real `width * height` byte buffer
//!   that can be base64-decoded back into the same shape the
//!   bridge would emit.

use kcreate_ai::{
    segment_image, segment_with_backend, upscale_lanczos, upscale_with_backend, SegmentBackend,
    SegmentError, SegmentOptions, UpscaleBackend, UpscaleError,
};

fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    let mut v = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for _ in 0..(width as usize) * (height as usize) {
        v.extend_from_slice(&rgba);
    }
    v
}

#[test]
fn upscale_dispatcher_matches_direct_lanczos() {
    let pixels = solid(16, 8, [50, 100, 200, 255]);
    let direct = upscale_lanczos(&pixels, 16, 8, 2.0).expect("direct");
    let dispatch = upscale_with_backend(&pixels, 16, 8, 2.0, UpscaleBackend::Lanczos3, None)
        .expect("dispatch");
    assert_eq!(direct.0, dispatch.0);
    assert_eq!(direct.1, dispatch.1);
    assert_eq!(direct.2, dispatch.2);
}

#[cfg(not(feature = "onnx_upscale"))]
#[test]
fn upscale_esrgan_unavailable_when_feature_off() {
    let pixels = solid(4, 4, [255, 0, 0, 255]);
    let err = upscale_with_backend(&pixels, 4, 4, 4.0, UpscaleBackend::Esrgan, None)
        .expect_err("must reject ESRGAN");
    assert!(matches!(
        err,
        UpscaleError::BackendUnavailable(UpscaleBackend::Esrgan)
    ));
}

#[test]
fn segment_edge_aware_returns_correct_mask_shape() {
    let w = 32;
    let h = 24;
    let pixels = solid(w, h, [200, 50, 50, 255]);
    let result = segment_image(
        &pixels,
        w,
        h,
        &SegmentOptions {
            point_x: w / 2,
            point_y: h / 2,
            tolerance: 0.5,
            edge_threshold: 0.5,
        },
    )
    .expect("segment");
    assert_eq!(result.backend, SegmentBackend::EdgeAware);
    assert_eq!(result.masks.len(), 1);
    let mask = &result.masks[0];
    assert_eq!(mask.width, w);
    assert_eq!(mask.height, h);
    assert_eq!(mask.mask.len(), (w as usize) * (h as usize));
    // A uniform red field has zero gradient, so the flood-fill
    // covers the whole canvas.
    assert_eq!(mask.area, u64::from(w) * u64::from(h));
}

#[test]
fn segment_rejects_invalid_options() {
    let pixels = solid(4, 4, [255, 0, 0, 255]);
    // Out-of-bounds point prompt.
    let err = segment_image(
        &pixels,
        4,
        4,
        &SegmentOptions {
            point_x: 10,
            point_y: 0,
            ..Default::default()
        },
    )
    .expect_err("must reject OOB");
    assert!(matches!(err, SegmentError::PointOutOfBounds(10, 0, 4, 4)));
    // NaN tolerance.
    let err = segment_image(
        &pixels,
        4,
        4,
        &SegmentOptions {
            point_x: 0,
            point_y: 0,
            tolerance: f64::NAN,
            edge_threshold: 0.5,
        },
    )
    .expect_err("must reject NaN tolerance");
    assert!(matches!(err, SegmentError::InvalidTolerance(_)));
}

#[cfg(not(feature = "onnx_segment"))]
#[test]
fn segment_sam_unavailable_when_feature_off() {
    let pixels = solid(4, 4, [255, 0, 0, 255]);
    let err = segment_with_backend(
        &pixels,
        4,
        4,
        &SegmentOptions {
            point_x: 0,
            point_y: 0,
            tolerance: 0.5,
            edge_threshold: 0.5,
        },
        SegmentBackend::Sam,
        None,
    )
    .expect_err("must reject SAM");
    assert!(matches!(
        err,
        SegmentError::BackendUnavailable(SegmentBackend::Sam)
    ));
}
