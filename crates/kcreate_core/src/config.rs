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
    pub ai_models_dir: PathBuf,
    pub low_resource_mode: bool,
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
}
