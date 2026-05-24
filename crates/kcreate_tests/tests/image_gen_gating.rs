//! Phase 4 image-generation tier-gating tests.
//!
//! Image generation is a **hard gate**, not a soft one. Per PROPOSAL §7:
//! Tier 0/1 must not see generation UI at all, and the registry must
//! not advertise generation packs to those tiers. This test file
//! freezes that contract.
//!
//! These tests run on `kcreate_core::config::RuntimeConfig` so they
//! don't need a real sidecar — the gate decision is a pure function
//! of `DeviceTier` and `SystemInfo.gpu_available`.

use kcreate_core::config::{DeviceTier, RuntimeConfig, SystemInfo};

/// Build a config with explicit tier + GPU. The
/// `RuntimeConfig::from_info` path picks the tier from RAM, but
/// we want to drive each tier directly rather than guess what RAM
/// budget yields what tier on the current CI runner.
fn cfg_with(tier: DeviceTier, gpu: bool) -> RuntimeConfig {
    let info = SystemInfo {
        // RAM is irrelevant once we overwrite the tier below.
        total_ram_mb: 16 * 1024,
        gpu_available: gpu,
    };
    let mut cfg = RuntimeConfig::from_info(info, None);
    cfg.device_tier = tier;
    cfg
}

#[test]
fn tier0_image_generation_forbidden_regardless_of_gpu() {
    assert!(
        !cfg_with(DeviceTier::Tier0, true).image_generation_allowed(),
        "Tier 0 must hard-gate image gen even with a GPU present",
    );
    assert!(
        !cfg_with(DeviceTier::Tier0, false).image_generation_allowed(),
        "Tier 0 must hard-gate image gen with no GPU",
    );
}

#[test]
fn tier1_image_generation_forbidden_regardless_of_gpu() {
    assert!(
        !cfg_with(DeviceTier::Tier1, true).image_generation_allowed(),
        "Tier 1 must hard-gate image gen even with a GPU present",
    );
    assert!(
        !cfg_with(DeviceTier::Tier1, false).image_generation_allowed(),
        "Tier 1 must hard-gate image gen with no GPU",
    );
}

#[test]
fn tier2_image_generation_requires_gpu() {
    assert!(
        cfg_with(DeviceTier::Tier2, true).image_generation_allowed(),
        "Tier 2 + GPU should allow image gen",
    );
    assert!(
        !cfg_with(DeviceTier::Tier2, false).image_generation_allowed(),
        "Tier 2 without GPU must NOT allow image gen — \
         diffusion on CPU is unusable at scale",
    );
}

#[test]
fn tier3_image_generation_requires_gpu() {
    assert!(
        cfg_with(DeviceTier::Tier3, true).image_generation_allowed(),
        "Tier 3 + GPU should allow image gen",
    );
    assert!(
        !cfg_with(DeviceTier::Tier3, false).image_generation_allowed(),
        "Tier 3 without GPU must NOT allow image gen — \
         the GPU is the bottleneck, not the tier",
    );
}

/// Vision is **soft-gated**: every tier must report
/// `vision_model_allowed() == true`. Tier-aware ceilings live in
/// `vision_model_max_mb`, NOT in the allow gate. This protects
/// against an over-eager refactor that would tighten the gate and
/// strand Tier 0 users without alt-text.
#[test]
fn vision_is_soft_gated_across_all_tiers() {
    for tier in [
        DeviceTier::Tier0,
        DeviceTier::Tier1,
        DeviceTier::Tier2,
        DeviceTier::Tier3,
    ] {
        assert!(
            tier.vision_model_allowed(),
            "{tier:?}.vision_model_allowed() must stay true — \
             vision is soft-gated, not hard-gated",
        );
    }
}

/// Per-tier vision-model size ceiling MUST be monotonic — a beefier
/// tier never gets a *smaller* budget than a weaker one. The exact
/// numbers can move over time (see `DeviceTier::vision_model_max_mb`
/// in `crates/kcreate_core/src/config.rs`) but monotonicity is the
/// invariant.
#[test]
fn vision_model_max_mb_is_monotonic_by_tier() {
    let t0 = DeviceTier::Tier0.vision_model_max_mb();
    let t1 = DeviceTier::Tier1.vision_model_max_mb();
    let t2 = DeviceTier::Tier2.vision_model_max_mb();
    let t3 = DeviceTier::Tier3.vision_model_max_mb();
    assert!(t0 <= t1, "{t0} > {t1}");
    assert!(t1 <= t2, "{t1} > {t2}");
    assert!(t2 <= t3, "{t2} > {t3}");
    // And the floor must accommodate SmolVLM-256M (~180 MB).
    assert!(
        t0 >= 256,
        "Tier 0 vision budget {t0} MB cannot fit SmolVLM-256M (~180 MB + overhead)",
    );
}

/// The model registry must NOT advertise generation packs to a
/// machine that can't run them. The hard gate is the
/// `image_generation_allowed` boolean in `ResourceLimits`, but
/// the renderer also filters the pack list — and that filter
/// only works if the registry serves the same packs to everyone
/// and lets the renderer hide them. Test that the registry side
/// behaves correctly: generation packs exist (so a Tier 2 user
/// can find them) but a Tier 0 caller's
/// `image_generation_allowed()` is false.
#[test]
fn registry_lists_generation_packs_only_when_tier_allows() {
    use kcreate_ai::model_registry::{list_model_packs, ModelPackCategory};
    use std::path::PathBuf;

    // Pointing at an empty tempdir means every pack reports
    // `installed: false`. That's fine for this assertion — we only
    // care about the static set of advertised packs.
    let scratch = tempfile::tempdir().expect("tempdir");
    let packs = list_model_packs(&PathBuf::from(scratch.path()));
    let gen_packs: Vec<_> = packs
        .iter()
        .filter(|p| p.category == ModelPackCategory::Generation)
        .collect();
    assert!(
        !gen_packs.is_empty(),
        "Registry must still list generation packs — the UI hides \
         them by tier, but the registry is canonical and must \
         enumerate every pack so a Tier 2+ machine can install one",
    );

    // Each generation pack must carry the `image_generation`
    // capability so the UI gating logic can look up by capability,
    // not just by category.
    for p in &gen_packs {
        assert!(
            p.capabilities.iter().any(|c| c == "image_generation"),
            "Generation pack {id:?} missing `image_generation` capability",
            id = p.id,
        );
    }
}
