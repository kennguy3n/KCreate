//! Runtime config — platform detection + device tiering.
//!
//! [`RuntimeConfig::detect`] probes the host system once and produces a
//! deterministic configuration the rest of the workspace consumes via
//! borrow. Tests construct a [`SystemInfo`] directly and call
//! [`DeviceTier::from_system_info`] so the tier-selection logic stays
//! pure and unit-testable.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Performance tier inferred from RAM + GPU availability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeviceTier {
    /// `< 8 GB` RAM, CPU only.
    Tier0,
    /// `>= 8 GB` RAM, integrated GPU.
    Tier1,
    /// `>= 16 GB` RAM, modern iGPU or discrete GPU.
    Tier2,
    /// `>= 32 GB` RAM, discrete GPU or Apple Silicon.
    Tier3,
}

impl DeviceTier {
    /// Maximum AI model size (MB) the tier will load. Tier 0 caps at
    /// ~Q4 7B-sized models; higher tiers allow proportionally more.
    #[must_use]
    pub const fn default_max_model_mb(self) -> u64 {
        match self {
            Self::Tier0 => 1500,
            Self::Tier1 => 4000,
            Self::Tier2 => 8000,
            Self::Tier3 => 16000,
        }
    }

    /// Whether GPU-backed rendering is permitted on this tier. The
    /// renderer's adapter request is still the authoritative check —
    /// this returns the *intent* of the device-tier policy. Tier 0
    /// stays on the CPU backend even if a GPU is technically
    /// available because the discrete VRAM budget would starve
    /// editing-side caches.
    #[must_use]
    pub const fn gpu_rendering_allowed(self) -> bool {
        matches!(self, Self::Tier1 | Self::Tier2 | Self::Tier3)
    }

    /// Derive a tier from RAM + GPU presence. Pure function; trivially
    /// testable.
    #[must_use]
    pub const fn from_system_info(info: &SystemInfo) -> Self {
        let gb = info.total_ram_mb / 1024;
        if gb >= 32 && info.gpu_available {
            Self::Tier3
        } else if gb >= 16 && info.gpu_available {
            Self::Tier2
        } else if gb >= 8 {
            Self::Tier1
        } else {
            Self::Tier0
        }
    }

    /// True for tiers that should default to low-resource mode.
    #[must_use]
    pub const fn defaults_to_low_resource(self) -> bool {
        matches!(self, Self::Tier0)
    }

    /// Suggested undo-history depth for this tier.
    #[must_use]
    pub const fn default_undo_depth(self) -> usize {
        match self {
            Self::Tier0 => 32,
            Self::Tier1 => 128,
            Self::Tier2 => 256,
            Self::Tier3 => 1024,
        }
    }

    /// Suggested raster cache budget (MB) for this tier.
    #[must_use]
    pub const fn default_raster_cache_mb(self) -> u64 {
        match self {
            Self::Tier0 => 64,
            Self::Tier1 => 256,
            Self::Tier2 => 1024,
            Self::Tier3 => 4096,
        }
    }
}

/// Operating system + CPU architecture combination `KCreate` supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Platform {
    MacOsIntel,
    MacOsAppleSilicon,
    WindowsX64,
    LinuxX64,
    LinuxArm64,
}

impl Platform {
    /// Detect the host platform from build-time `cfg` flags.
    #[must_use]
    pub const fn current() -> Self {
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        {
            Self::MacOsIntel
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            Self::MacOsAppleSilicon
        }
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            Self::WindowsX64
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            Self::LinuxX64
        }
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        {
            Self::LinuxArm64
        }
        #[cfg(not(any(
            all(target_os = "macos", target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "windows", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "aarch64"),
        )))]
        {
            // Fall back to a reasonable default for unusual hosts; we
            // still try to *run* there, we just won't claim official
            // support.
            Self::LinuxX64
        }
    }
}

/// Pure data describing the host. Constructed from a real probe in
/// [`RuntimeConfig::detect`] or by tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemInfo {
    pub total_ram_mb: u64,
    pub gpu_available: bool,
}

/// Resolved runtime configuration. One per process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub platform: Platform,
    pub device_tier: DeviceTier,
    pub total_ram_mb: u64,
    pub gpu_available: bool,
    pub gpu_name: Option<String>,
    pub max_undo_depth: usize,
    pub tile_size: u32,
    pub max_raster_cache_mb: u64,
    pub max_model_mb: u64,
    pub ai_models_dir: PathBuf,
    pub low_resource_mode: bool,
}

impl RuntimeConfig {
    /// True when low-resource mode is active (Tier 0 implicitly, or
    /// any tier that has had the flag flipped explicitly via
    /// [`Self::set_low_resource`]).
    #[must_use]
    pub const fn is_low_resource(&self) -> bool {
        self.low_resource_mode
    }

    /// Effective undo depth: the minimum of the configured depth and
    /// the tier default. Low-resource mode further halves the
    /// effective depth so the editing log stays bounded on tiny
    /// devices.
    #[must_use]
    pub fn effective_undo_depth(&self) -> usize {
        let base = self
            .max_undo_depth
            .min(self.device_tier.default_undo_depth());
        if self.low_resource_mode {
            (base / 2).max(8)
        } else {
            base
        }
    }

    /// Effective raster cache budget in MB. Low-resource mode halves
    /// the configured budget to leave headroom for the rest of the
    /// host process.
    #[must_use]
    pub fn effective_raster_cache_mb(&self) -> u64 {
        let base = self
            .max_raster_cache_mb
            .min(self.device_tier.default_raster_cache_mb());
        if self.low_resource_mode {
            (base / 2).max(16)
        } else {
            base
        }
    }

    /// Effective maximum model size in MB.
    #[must_use]
    pub fn effective_max_model_mb(&self) -> u64 {
        let base = self
            .max_model_mb
            .min(self.device_tier.default_max_model_mb());
        if self.low_resource_mode {
            (base * 2) / 3
        } else {
            base
        }
    }

    /// GPU rendering is allowed when (a) the tier permits it, (b) a
    /// GPU was detected, and (c) low-resource mode is off.
    #[must_use]
    pub const fn gpu_rendering_allowed(&self) -> bool {
        self.gpu_available && self.device_tier.gpu_rendering_allowed() && !self.low_resource_mode
    }

    /// Flip the low-resource flag in place. Tier 0 always stays at
    /// `true` regardless of the request because the tier defaults
    /// require it.
    pub fn set_low_resource(&mut self, enabled: bool) {
        self.low_resource_mode = enabled || self.device_tier.defaults_to_low_resource();
    }
}

impl RuntimeConfig {
    /// Probe the host system and build a runtime config. GPU detection
    /// is intentionally conservative — the renderer itself will fall
    /// back to the CPU backend if it cannot acquire a wgpu adapter, so
    /// "no GPU" here is just a hint that biases the device-tier
    /// classification.
    #[must_use]
    pub fn detect() -> Self {
        // `sys_info::mem_info().total` is in kB on every platform we
        // support (Linux, macOS, Windows) — the crate normalises the
        // platform-specific source (sysinfo/GlobalMemoryStatusEx/
        // sysctl) into a uniform kB-typed `MemInfo`. Divide by 1024 to
        // get MB.
        let total_ram_mb = sys_info::mem_info().map_or(0, |m| m.total / 1024);
        let gpu_available = probe_gpu_available();
        let info = SystemInfo {
            total_ram_mb,
            gpu_available,
        };
        Self::from_info(info, gpu_name_hint())
    }

    /// Build a config from explicit `SystemInfo` plus an optional GPU
    /// vendor/name hint. Used by `detect` and by tests.
    #[must_use]
    pub fn from_info(info: SystemInfo, gpu_name: Option<String>) -> Self {
        let platform = Platform::current();
        let device_tier = DeviceTier::from_system_info(&info);
        let max_undo_depth = device_tier.default_undo_depth();
        let max_raster_cache_mb = device_tier.default_raster_cache_mb();
        let max_model_mb = device_tier.default_max_model_mb();
        let low_resource_mode = device_tier.defaults_to_low_resource();
        Self {
            platform,
            device_tier,
            total_ram_mb: info.total_ram_mb,
            gpu_available: info.gpu_available,
            gpu_name,
            max_undo_depth,
            tile_size: 256,
            max_raster_cache_mb,
            max_model_mb,
            ai_models_dir: default_ai_models_dir(),
            low_resource_mode,
        }
    }
}

#[allow(clippy::missing_const_for_fn)]
fn probe_gpu_available() -> bool {
    // We treat the presence of *any* of the platform-specific
    // GPU-API library paths as an availability hint. The renderer's
    // actual `wgpu::Instance::request_adapter` call is the
    // authoritative check — we don't link to it here just to avoid
    // pulling the renderer crate into `kcreate_core`.
    #[cfg(target_os = "macos")]
    {
        // Metal is part of macOS — if you're on macOS, you have Metal.
        true
    }
    #[cfg(target_os = "windows")]
    {
        // D3D12 is part of Windows 10+. We trust this without probing.
        true
    }
    #[cfg(target_os = "linux")]
    {
        // On Linux, look for at least one common GPU runtime library.
        for path in [
            "/usr/lib/x86_64-linux-gnu/libvulkan.so.1",
            "/usr/lib64/libvulkan.so.1",
            "/usr/lib/aarch64-linux-gnu/libvulkan.so.1",
            "/usr/lib/x86_64-linux-gnu/libGL.so.1",
        ] {
            if std::path::Path::new(path).exists() {
                return true;
            }
        }
        false
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        false
    }
}

const fn gpu_name_hint() -> Option<String> {
    // We don't ship a heavyweight PCI/IOKit query in `kcreate_core`.
    // The renderer crate, when it acquires its adapter, fills this in
    // via `RuntimeConfig::gpu_name = Some(adapter.get_info().name)`.
    None
}

fn default_ai_models_dir() -> PathBuf {
    directories_dir().map_or_else(
        || PathBuf::from(".kcreate").join("models"),
        |dir| dir.join("KCreate").join("models"),
    )
}

fn directories_dir() -> Option<PathBuf> {
    // `directories` would add a dep; this is a 4-line direct
    // implementation for the three OSes we actually ship to.
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(PathBuf::from)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_classification_boundaries() {
        let info = |ram_gb: u64, gpu: bool| SystemInfo {
            total_ram_mb: ram_gb * 1024,
            gpu_available: gpu,
        };
        assert_eq!(
            DeviceTier::from_system_info(&info(4, false)),
            DeviceTier::Tier0
        );
        assert_eq!(
            DeviceTier::from_system_info(&info(4, true)),
            DeviceTier::Tier0
        );
        assert_eq!(
            DeviceTier::from_system_info(&info(8, false)),
            DeviceTier::Tier1
        );
        assert_eq!(
            DeviceTier::from_system_info(&info(8, true)),
            DeviceTier::Tier1
        );
        assert_eq!(
            DeviceTier::from_system_info(&info(16, true)),
            DeviceTier::Tier2
        );
        assert_eq!(
            DeviceTier::from_system_info(&info(16, false)),
            DeviceTier::Tier1
        );
        assert_eq!(
            DeviceTier::from_system_info(&info(32, true)),
            DeviceTier::Tier3
        );
        assert_eq!(
            DeviceTier::from_system_info(&info(64, true)),
            DeviceTier::Tier3
        );
        assert_eq!(
            DeviceTier::from_system_info(&info(64, false)),
            DeviceTier::Tier1
        );
    }

    #[test]
    fn tier_undo_depth_monotonic() {
        let depths: Vec<usize> = [
            DeviceTier::Tier0,
            DeviceTier::Tier1,
            DeviceTier::Tier2,
            DeviceTier::Tier3,
        ]
        .into_iter()
        .map(DeviceTier::default_undo_depth)
        .collect();
        assert!(depths.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn tier_raster_cache_monotonic() {
        let budgets: Vec<u64> = [
            DeviceTier::Tier0,
            DeviceTier::Tier1,
            DeviceTier::Tier2,
            DeviceTier::Tier3,
        ]
        .into_iter()
        .map(DeviceTier::default_raster_cache_mb)
        .collect();
        assert!(budgets.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn low_resource_mode_only_on_tier0() {
        assert!(DeviceTier::Tier0.defaults_to_low_resource());
        assert!(!DeviceTier::Tier1.defaults_to_low_resource());
        assert!(!DeviceTier::Tier2.defaults_to_low_resource());
        assert!(!DeviceTier::Tier3.defaults_to_low_resource());
    }

    #[test]
    fn from_info_produces_consistent_config() {
        let info = SystemInfo {
            total_ram_mb: 16 * 1024,
            gpu_available: true,
        };
        let cfg = RuntimeConfig::from_info(info, Some("Test GPU".into()));
        assert_eq!(cfg.device_tier, DeviceTier::Tier2);
        assert_eq!(cfg.total_ram_mb, 16 * 1024);
        assert!(cfg.gpu_available);
        assert_eq!(cfg.gpu_name.as_deref(), Some("Test GPU"));
        assert_eq!(cfg.max_undo_depth, 256);
        assert_eq!(cfg.tile_size, 256);
        assert!(!cfg.low_resource_mode);
    }

    #[test]
    fn from_info_tier0_enables_low_resource_mode() {
        let info = SystemInfo {
            total_ram_mb: 4 * 1024,
            gpu_available: false,
        };
        let cfg = RuntimeConfig::from_info(info, None);
        assert_eq!(cfg.device_tier, DeviceTier::Tier0);
        assert!(cfg.low_resource_mode);
        assert_eq!(cfg.max_undo_depth, 32);
    }

    #[test]
    fn detect_does_not_panic() {
        // Real probe — values vary by host. Just sanity-check fields.
        let cfg = RuntimeConfig::detect();
        assert!(cfg.tile_size >= 128);
        assert!(cfg.max_undo_depth > 0);
    }

    #[test]
    fn current_platform_is_supported() {
        let p = Platform::current();
        let _ = format!("{p:?}");
    }

    #[test]
    fn tier_max_model_monotonic() {
        let m: Vec<u64> = [
            DeviceTier::Tier0,
            DeviceTier::Tier1,
            DeviceTier::Tier2,
            DeviceTier::Tier3,
        ]
        .into_iter()
        .map(DeviceTier::default_max_model_mb)
        .collect();
        assert!(m.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn gpu_rendering_allowed_only_above_tier0() {
        assert!(!DeviceTier::Tier0.gpu_rendering_allowed());
        assert!(DeviceTier::Tier1.gpu_rendering_allowed());
        assert!(DeviceTier::Tier2.gpu_rendering_allowed());
        assert!(DeviceTier::Tier3.gpu_rendering_allowed());
    }

    fn cfg_for(ram_gb: u64, gpu: bool) -> RuntimeConfig {
        RuntimeConfig::from_info(
            SystemInfo {
                total_ram_mb: ram_gb * 1024,
                gpu_available: gpu,
            },
            None,
        )
    }

    #[test]
    fn effective_limits_respect_low_resource() {
        let mut cfg = cfg_for(16, true); // Tier2, no LR
        let undo_full = cfg.effective_undo_depth();
        let raster_full = cfg.effective_raster_cache_mb();
        let model_full = cfg.effective_max_model_mb();
        assert!(cfg.gpu_rendering_allowed());
        cfg.set_low_resource(true);
        assert!(cfg.is_low_resource());
        assert!(cfg.effective_undo_depth() < undo_full);
        assert!(cfg.effective_raster_cache_mb() < raster_full);
        assert!(cfg.effective_max_model_mb() < model_full);
        assert!(!cfg.gpu_rendering_allowed());
    }

    #[test]
    fn tier0_cannot_disable_low_resource() {
        let mut cfg = cfg_for(4, false); // Tier0
        assert!(cfg.is_low_resource());
        cfg.set_low_resource(false);
        // Tier0 is forced back into low-resource regardless.
        assert!(cfg.is_low_resource());
    }

    #[test]
    fn effective_undo_depth_clamps_to_floor() {
        // Even with low-resource halving, the floor is 8.
        let mut cfg = cfg_for(4, false);
        assert_eq!(cfg.device_tier, DeviceTier::Tier0);
        cfg.max_undo_depth = 4;
        assert!(cfg.effective_undo_depth() >= 8);
    }
}
