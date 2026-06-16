//! Native-canvas presentation path — Phase 1, Block A, Task 4–5.
//!
//! Wraps the raw window handle bytes Electron hands us via
//! `BrowserWindow::getNativeWindowHandle()` into the
//! [`raw_window_handle::HasWindowHandle`] / [`HasDisplayHandle`]
//! shape required by [`kcreate_renderer::NativeSurface::from_window`].
//!
//! This is the **only** module in the project that uses `unsafe`. It
//! lives behind the `native_canvas` cargo feature so default builds
//! stay `unsafe`-free.
//!
//! Safety contract (mirrored verbatim into the SAFETY comment on
//! every `unsafe` block below):
//!
//! - Electron's `BrowserWindow::getNativeWindowHandle()` returns a
//!   `Buffer` whose contents are a real OS window handle:
//!     - macOS: `NSView*` (8 bytes, little-endian on aarch64 / x86_64)
//!     - Windows: `HWND` (8 bytes on x86_64, 4 bytes on Win32)
//!     - Linux X11: `Window` XID (4 bytes, little-endian)
//!     - Linux Wayland: `wl_surface*` (8 bytes)
//! - The BrowserWindow stays alive for the entire editing session —
//!   the renderer (which owns the `Arc<PlatformHandle>` we build
//!   here) is destroyed before window close, so the borrow is valid
//!   for the surface's lifetime.
//! - We never dereference the handle ourselves; we only ferry it to
//!   wgpu via [`raw_window_handle::WindowHandle::borrow_raw`], which
//!   itself is the unsafe-asserting boundary.
//!
//! Lifecycle enforcement: the Electron main process wires the
//! BrowserWindow's `close` event (which fires *before* the OS
//! resource is destroyed) to `bridge.rendererSwitchOffscreen()`. That
//! call detaches the native surface and drops the
//! `Arc<PlatformHandle>` here, so no `wgpu::Surface` ever survives
//! its underlying OS window. See `apps/desktop/main/src/main.ts`
//! `createWindow` (`win.on("close", ...)`). This is what makes the
//! `Send + Sync` impls below sound in practice; the contract is not
//! a hope, it is enforced by the host.

#![cfg(feature = "native_canvas")]

use std::ptr::NonNull;
use std::sync::Arc;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WindowHandle,
};
use thiserror::Error;

/// Which OS path the bridge interpreted the handle bytes as. Exposed
/// so the host UI can show "Native (X11)" / "Native (Wayland)" /
/// "Native (Win32)" / "Native (AppKit)" badges, and so tests can
/// validate the per-platform parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePlatform {
    AppKit,
    Win32,
    X11,
    Wayland,
}

impl NativePlatform {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AppKit => "appkit",
            Self::Win32 => "win32",
            Self::X11 => "x11",
            Self::Wayland => "wayland",
        }
    }
}

/// Errors specific to the native-canvas wrapper.
#[derive(Debug, Error)]
pub enum NativeCanvasError {
    #[error("handle bytes too short: got {got}, need {need} for {platform}")]
    HandleTooShort {
        got: usize,
        need: usize,
        platform: &'static str,
    },
    #[error("handle bytes encode a null pointer ({platform})")]
    NullHandle { platform: &'static str },
    #[error("unsupported platform — no native handle interpretation compiled in")]
    UnsupportedPlatform,
    /// We detected a Wayland session but the bridge does not yet have a
    /// real `wl_display*` connection. Returning this signals the host
    /// to keep using the offscreen presentation path. See the comment
    /// on `wayland_handle` below for the longer-term fix.
    #[error(
        "wayland native presentation requires a real wl_display* — \
         not yet wired through Electron's BrowserWindow handle, falling \
         back to offscreen presentation"
    )]
    WaylandNotYetSupported,
}

/// Owned platform handle. Implements both `HasWindowHandle` and
/// `HasDisplayHandle`; the embedded `RawWindowHandle` /
/// `RawDisplayHandle` are inert plain-data wrappers around an
/// integer / pointer, so the struct itself is `Send + Sync` even
/// though the handle ultimately points at OS state.
#[derive(Debug)]
pub struct PlatformHandle {
    window: RawWindowHandle,
    display: RawDisplayHandle,
    platform: NativePlatform,
}

// `RawWindowHandle` and `RawDisplayHandle` are not `Send + Sync` by
// default (they contain raw pointers). The unsafe `Send + Sync` impls
// here assert the same invariant the `wgpu::Instance::create_surface`
// API requires: the handle bytes refer to an OS-owned resource whose
// lifetime exceeds the surface, and we never mutate or dereference
// the handle from any thread. Electron pins the BrowserWindow for
// the entire editing session, which satisfies this.

// SAFETY: see the contract at the top of this file. The wrapped
// handle is a pointer / XID owned by the Electron BrowserWindow,
// which outlives the renderer.
unsafe impl Send for PlatformHandle {}
// SAFETY: see the contract at the top of this file. The wrapped
// handle is read-only as far as Rust is concerned and never
// dereferenced from this code; wgpu interprets it.
unsafe impl Sync for PlatformHandle {}

impl PlatformHandle {
    #[must_use]
    pub const fn platform(&self) -> NativePlatform {
        self.platform
    }
}

impl HasWindowHandle for PlatformHandle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: the raw handle was extracted from Electron's
        // `BrowserWindow::getNativeWindowHandle()`. The BrowserWindow
        // is held alive by the Electron main process for the entire
        // session and the surface using this handle is destroyed
        // before window close (`renderer_switch_offscreen` /
        // `kcreate_bridge::state::shutdown`).
        Ok(unsafe { WindowHandle::borrow_raw(self.window) })
    }
}

impl HasDisplayHandle for PlatformHandle {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: same contract as `window_handle`. The display
        // handle is constant for the session (the X11 display
        // connection / Wayland display / Win32 has no per-window
        // display, so we use the `Windows` variant).
        Ok(unsafe { DisplayHandle::borrow_raw(self.display) })
    }
}

/// Interpret a slab of bytes from Electron as a native window
/// handle. The interpretation is platform-specific:
///
/// - macOS: 8 bytes, little-endian `NSView*` (treated as `NSView`
///   via `AppKitWindowHandle`).
/// - Windows: 8 bytes (x64) or 4 bytes (Win32) `HWND`.
/// - Linux X11: 4 bytes, little-endian `Window` XID.
/// - Linux Wayland: 8 bytes, little-endian `wl_surface*`.
///
/// The display variant defaults to "no specific display" where the
/// platform allows it (Win32 and AppKit do; X11 and Wayland use
/// their respective display variants with a null connection
/// pointer, which `wgpu` resolves via the default connection it
/// already maintains).
pub fn handle_from_bytes(bytes: &[u8]) -> Result<PlatformHandle, NativeCanvasError> {
    interpret_bytes(bytes)
}

#[cfg(target_os = "macos")]
fn interpret_bytes(bytes: &[u8]) -> Result<PlatformHandle, NativeCanvasError> {
    use raw_window_handle::{AppKitDisplayHandle, AppKitWindowHandle};
    let ptr = read_ptr(bytes, "macos")?;
    let nn = NonNull::new(ptr).ok_or(NativeCanvasError::NullHandle { platform: "macos" })?;
    let window = RawWindowHandle::AppKit(AppKitWindowHandle::new(nn));
    let display = RawDisplayHandle::AppKit(AppKitDisplayHandle::new());
    Ok(PlatformHandle {
        window,
        display,
        platform: NativePlatform::AppKit,
    })
}

#[cfg(target_os = "windows")]
fn interpret_bytes(bytes: &[u8]) -> Result<PlatformHandle, NativeCanvasError> {
    use std::num::NonZeroIsize;

    use raw_window_handle::{Win32WindowHandle, WindowsDisplayHandle};
    let raw = read_isize(bytes, "windows")?;
    let nz = NonZeroIsize::new(raw).ok_or(NativeCanvasError::NullHandle {
        platform: "windows",
    })?;
    let window = RawWindowHandle::Win32(Win32WindowHandle::new(nz));
    let display = RawDisplayHandle::Windows(WindowsDisplayHandle::new());
    Ok(PlatformHandle {
        window,
        display,
        platform: NativePlatform::Win32,
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn interpret_bytes(bytes: &[u8]) -> Result<PlatformHandle, NativeCanvasError> {
    // Linux can be either X11 or Wayland. We resolve which by
    // consulting two environment variables in priority order. The
    // raw handle bytes are NOT used as a tie-breaker, because an
    // X11 XID is conceptually a 32-bit unsigned int but Electron's
    // `Buffer` slabs may be zero-padded to 8 bytes on 64-bit
    // systems, making byte-length unreliable as a discriminator
    // (Devin Review PR #5 follow-up: rejects the older
    // `bytes.len() == 8` heuristic that could mis-route a padded
    // X11 XID into the Wayland path).
    //
    // Resolution order:
    //   1. `XDG_SESSION_TYPE=x11` or `=wayland` is authoritative.
    //      This is the standard session-manager-provided signal.
    //   2. If `XDG_SESSION_TYPE` is unset or has any other value, we
    //      fall back to `WAYLAND_DISPLAY`. Wayland clients are
    //      required by the protocol to honour this env var; its
    //      presence is a strong signal that the user is on a
    //      Wayland compositor.
    //   3. With neither signal present, we default to X11. The X11
    //      path validates the XID without dereferencing it, so a
    //      mistaken X11 default on a Wayland session fails cleanly
    //      ("null handle") rather than touching memory.
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    if session.eq_ignore_ascii_case("x11") {
        return x11_handle(bytes);
    }
    if session.eq_ignore_ascii_case("wayland") {
        return wayland_handle(bytes);
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return wayland_handle(bytes);
    }
    x11_handle(bytes)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn x11_handle(bytes: &[u8]) -> Result<PlatformHandle, NativeCanvasError> {
    use raw_window_handle::{XlibDisplayHandle, XlibWindowHandle};
    if bytes.len() < 4 {
        return Err(NativeCanvasError::HandleTooShort {
            got: bytes.len(),
            need: 4,
            platform: "x11",
        });
    }
    let xid_le = [bytes[0], bytes[1], bytes[2], bytes[3]];
    let xid = u32::from_le_bytes(xid_le);
    if xid == 0 {
        return Err(NativeCanvasError::NullHandle { platform: "x11" });
    }
    let mut win = XlibWindowHandle::new(u64::from(xid));
    win.visual_id = 0;
    let display = XlibDisplayHandle::new(None, 0);
    Ok(PlatformHandle {
        window: RawWindowHandle::Xlib(win),
        display: RawDisplayHandle::Xlib(display),
        platform: NativePlatform::X11,
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn wayland_handle(bytes: &[u8]) -> Result<PlatformHandle, NativeCanvasError> {
    // Per Devin Review BUG-0001 (PR #5): the earlier draft constructed
    // a `WaylandDisplayHandle` using `NonNull::dangling()` as the
    // `wl_display*`. That pointer would be dereferenced by wgpu the
    // moment it tried to talk to the compositor, causing a segfault.
    //
    // The fully-correct fix requires either (a) an FFI call to
    // `wl_display_connect(NULL)` (which would add a hard dependency
    // on `libwayland-client.so.0` to the bridge) or (b) Electron
    // passing both the `wl_surface*` AND the `wl_display*` into the
    // bridge as separate buffers. Neither is wired up yet — and the
    // native_canvas feature is itself opt-in, off by default, with
    // the offscreen path as the universal fallback (see
    // `crates/kcreate_renderer/src/presenter.rs`).
    //
    // Until that wiring exists, the safe behaviour on a Wayland
    // session is to refuse to attach a native surface and let the
    // renderer keep using the offscreen path. We still validate the
    // surface bytes so the caller gets a clear "handle too short /
    // null" error if Electron handed us garbage, rather than the
    // less-actionable `WaylandNotYetSupported` after a length pass.
    let ptr = read_ptr(bytes, "wayland")?;
    let _: NonNull<std::ffi::c_void> = NonNull::new(ptr).ok_or(NativeCanvasError::NullHandle {
        platform: "wayland",
    })?;
    Err(NativeCanvasError::WaylandNotYetSupported)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", unix)))]
fn interpret_bytes(_bytes: &[u8]) -> Result<PlatformHandle, NativeCanvasError> {
    Err(NativeCanvasError::UnsupportedPlatform)
}

#[allow(dead_code)]
fn read_ptr(
    bytes: &[u8],
    platform: &'static str,
) -> Result<*mut std::ffi::c_void, NativeCanvasError> {
    if bytes.len() < 8 {
        return Err(NativeCanvasError::HandleTooShort {
            got: bytes.len(),
            need: 8,
            platform,
        });
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[..8]);
    let raw = u64::from_le_bytes(buf);
    // Cast u64 → usize → pointer. On 32-bit targets the high half of
    // the u64 is ignored (Electron Buffer slabs are zero-padded).
    #[allow(clippy::cast_possible_truncation)]
    let as_usize = raw as usize;
    Ok(as_usize as *mut std::ffi::c_void)
}

#[allow(dead_code)]
fn read_isize(bytes: &[u8], platform: &'static str) -> Result<isize, NativeCanvasError> {
    let need = std::mem::size_of::<isize>();
    if bytes.len() < need {
        return Err(NativeCanvasError::HandleTooShort {
            got: bytes.len(),
            need,
            platform,
        });
    }
    if need == 8 {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&bytes[..8]);
        #[allow(clippy::cast_possible_wrap)]
        Ok(i64::from_le_bytes(buf) as isize)
    } else {
        let mut buf = [0u8; 4];
        buf.copy_from_slice(&bytes[..4]);
        #[allow(clippy::cast_possible_wrap)]
        Ok(i32::from_le_bytes(buf) as isize)
    }
}

/// Wrap raw handle bytes into an `Arc<PlatformHandle>` suitable for
/// `kcreate_renderer::RenderContext::create_native_surface`.
pub fn wrap_handle(bytes: &[u8]) -> Result<Arc<PlatformHandle>, NativeCanvasError> {
    Ok(Arc::new(handle_from_bytes(bytes)?))
}

// -----------------------------------------------------------------------------
// Memory-mapped region wrappers — shared-memory frame handoff.
//
// `memmap2::MmapOptions::map` / `map_mut` are `unsafe fn` because the
// kernel mapping aliases a file whose bytes another process could
// concurrently change in ways the Rust abstract machine cannot model.
// These two wrappers are the **only** new `unsafe` in the shared-memory
// present path; every higher layer (`shared_present.rs`) operates purely
// on the safe `&[u8]` / `&mut [u8]` views handed out here, so the unsafe
// surface stays pinned inside this module behind the `native_canvas`
// feature, exactly as the crate's `unsafe_code` policy requires.
//
// Safety contract shared by both wrappers:
//
// - The mapping length is fixed to `len` (the length the file was
//   `set_len`'d to by [`MappedRegion::create`]). The publisher never
//   truncates or grows the file while a mapping is live, so the mapped
//   pages stay backed for the whole lifetime of the wrapper.
// - We hand out byte slices only; callers (`shared_present.rs`) read and
//   write through a single-writer / many-reader **seqlock**, so a reader
//   that observes a half-written frame retries rather than acting on torn
//   bytes. Tearing therefore degrades to a retry, never to UB.
// - The wrapper owns the mapping; dropping it unmaps. The backing
//   `File` is kept alive alongside the mapping by the owners in
//   `shared_present.rs` (Windows keeps the mapping valid via a duplicated
//   handle, but holding the `File` is belt-and-suspenders on every OS).

use std::fs::File;
use std::io;

use memmap2::{Mmap, MmapMut, MmapOptions};

/// A writable shared-memory mapping (publisher side).
#[derive(Debug)]
pub struct MappedRegion {
    mmap: MmapMut,
}

impl MappedRegion {
    /// Map `file` (already `set_len`'d to `len`) read-write.
    pub fn create(file: &File, len: usize) -> io::Result<Self> {
        // SAFETY: see the module-level contract above. `file` is owned
        // by the publisher, sized to exactly `len`, and never truncated
        // while this mapping is live; all access goes through the
        // seqlock in `shared_present.rs`.
        let mmap = unsafe { MmapOptions::new().len(len).map_mut(file)? };
        Ok(Self { mmap })
    }

    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.mmap
    }

    #[must_use]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.mmap
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.mmap.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mmap.is_empty()
    }
}

/// A read-only shared-memory mapping (reader side).
#[derive(Debug)]
pub struct MappedRegionRo {
    mmap: Mmap,
}

impl MappedRegionRo {
    /// Map `file` read-only over `len` bytes.
    pub fn open(file: &File, len: usize) -> io::Result<Self> {
        // SAFETY: see the module-level contract above. The reader opens
        // the publisher's file read-only; the publisher guarantees the
        // file is sized to `len` and is never truncated while readers
        // hold a mapping, and all reads go through the seqlock retry.
        let mmap = unsafe { MmapOptions::new().len(len).map(file)? };
        Ok(Self { mmap })
    }

    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.mmap
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.mmap.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mmap.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
mod tests {
    use serial_test::serial;

    use super::*;

    #[test]
    fn rejects_short_handles() {
        let err = handle_from_bytes(&[0u8; 2]).expect_err("too short");
        assert!(matches!(err, NativeCanvasError::HandleTooShort { .. }));
    }

    #[test]
    fn rejects_null_handle() {
        // 8 zero bytes on platforms that need 8 bytes; 4 zero bytes on
        // X11. Either way, the interpreter rejects.
        let err = handle_from_bytes(&[0u8; 8]).expect_err("null");
        assert!(matches!(err, NativeCanvasError::NullHandle { .. }));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    #[serial]
    fn x11_round_trips_xid() {
        // Force X11 path by setting XDG_SESSION_TYPE=x11. We can't
        // unset env vars cleanly inside a #[test] without affecting
        // other tests; set to x11 for the duration.
        let prev = std::env::var("XDG_SESSION_TYPE").ok();
        // SAFETY: in tests we accept the small race risk of mutating
        // a process-global env var; the build's test harness runs
        // serially for `native_canvas` tests due to the env mutation.
        unsafe { std::env::set_var("XDG_SESSION_TYPE", "x11") };
        let xid = 0x0123_4567u32;
        let bytes = xid.to_le_bytes();
        let handle = handle_from_bytes(&bytes).expect("x11 wrap");
        assert_eq!(handle.platform(), NativePlatform::X11);
        match handle.window {
            RawWindowHandle::Xlib(h) => assert_eq!(h.window, u64::from(xid)),
            other => panic!("expected Xlib, got {other:?}"),
        }
        match prev {
            Some(v) => {
                // SAFETY: see above; restoring the prior value.
                unsafe { std::env::set_var("XDG_SESSION_TYPE", v) };
            }
            None => {
                // SAFETY: see above; restoring an unset env var.
                unsafe { std::env::remove_var("XDG_SESSION_TYPE") };
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_wraps_8_byte_pointer() {
        let ptr_val: u64 = 0xDEAD_BEEF_CAFE_BABE;
        let bytes = ptr_val.to_le_bytes();
        let handle = handle_from_bytes(&bytes).expect("appkit wrap");
        assert_eq!(handle.platform(), NativePlatform::AppKit);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_wraps_isize_pointer() {
        let mut bytes = [0u8; std::mem::size_of::<isize>()];
        bytes[0] = 0x41;
        let handle = handle_from_bytes(&bytes).expect("win32 wrap");
        assert_eq!(handle.platform(), NativePlatform::Win32);
    }

    /// Devin Review BUG-0001 regression. An 8-byte buffer on a session
    /// that the bridge interprets as Wayland (either by
    /// `XDG_SESSION_TYPE=wayland` or by length fallback) must return
    /// `WaylandNotYetSupported` rather than constructing a handle with
    /// a dangling display pointer. The host is expected to keep using
    /// the offscreen presentation path.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    #[serial]
    fn wayland_session_refuses_until_wl_display_wired() {
        let prev = std::env::var("XDG_SESSION_TYPE").ok();
        // SAFETY: see x11_round_trips_xid.
        unsafe { std::env::set_var("XDG_SESSION_TYPE", "wayland") };
        let ptr_val: u64 = 0xDEAD_BEEF_CAFE_BABE;
        let bytes = ptr_val.to_le_bytes();
        let err = handle_from_bytes(&bytes).expect_err("wayland refuses");
        assert!(matches!(err, NativeCanvasError::WaylandNotYetSupported));
        match prev {
            Some(v) => {
                // SAFETY: see above; restoring the prior value.
                unsafe { std::env::set_var("XDG_SESSION_TYPE", v) };
            }
            None => {
                // SAFETY: see above; restoring an unset env var.
                unsafe { std::env::remove_var("XDG_SESSION_TYPE") };
            }
        }
    }

    /// Devin Review BUG-0002 regression. An explicit `XDG_SESSION_TYPE=x11`
    /// must route to `x11_handle` even when the byte slab is 8 bytes
    /// (zero-padded XID, which Electron may produce). Previously the
    /// `|| bytes.len() == 8` short-circuit would shunt the buffer to
    /// the Wayland path and misinterpret the XID as a `wl_surface*`.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    #[serial]
    fn explicit_x11_session_overrides_byte_length_heuristic() {
        let prev = std::env::var("XDG_SESSION_TYPE").ok();
        // SAFETY: see x11_round_trips_xid.
        unsafe { std::env::set_var("XDG_SESSION_TYPE", "x11") };
        let xid: u32 = 0x0042_4242;
        let mut bytes = [0u8; 8];
        bytes[..4].copy_from_slice(&xid.to_le_bytes());
        let handle = handle_from_bytes(&bytes).expect("x11 wrap with zero-padded buffer");
        assert_eq!(handle.platform(), NativePlatform::X11);
        match handle.window {
            RawWindowHandle::Xlib(h) => assert_eq!(h.window, u64::from(xid)),
            other => panic!("expected Xlib, got {other:?}"),
        }
        match prev {
            Some(v) => {
                // SAFETY: see above; restoring the prior value.
                unsafe { std::env::set_var("XDG_SESSION_TYPE", v) };
            }
            None => {
                // SAFETY: see above; restoring an unset env var.
                unsafe { std::env::remove_var("XDG_SESSION_TYPE") };
            }
        }
    }

    /// Devin Review PR #5 follow-up. With `XDG_SESSION_TYPE` unset
    /// (some minimal session managers omit it) but `WAYLAND_DISPLAY`
    /// present, we should route to the Wayland path. Conversely, with
    /// neither variable set, an 8-byte buffer must NOT be misrouted
    /// to Wayland (which was the old `bytes.len() == 8` heuristic
    /// failure mode): the safer default is X11.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    #[serial]
    fn wayland_display_env_routes_to_wayland_when_session_unset() {
        let prev_session = std::env::var("XDG_SESSION_TYPE").ok();
        let prev_display = std::env::var("WAYLAND_DISPLAY").ok();
        // SAFETY: process-global env mutation; native_canvas tests
        // are intentionally run as a single suite with no parallelism
        // around env mutation.
        unsafe {
            std::env::remove_var("XDG_SESSION_TYPE");
            std::env::set_var("WAYLAND_DISPLAY", "wayland-0");
        }
        let ptr_val: u64 = 0xDEAD_BEEF_CAFE_BABE;
        let bytes = ptr_val.to_le_bytes();
        let err = handle_from_bytes(&bytes).expect_err("wayland refuses");
        assert!(matches!(err, NativeCanvasError::WaylandNotYetSupported));
        // Restore env.
        unsafe {
            match prev_session {
                Some(v) => std::env::set_var("XDG_SESSION_TYPE", v),
                None => std::env::remove_var("XDG_SESSION_TYPE"),
            }
            match prev_display {
                Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
                None => std::env::remove_var("WAYLAND_DISPLAY"),
            }
        }
    }

    /// Devin Review PR #5 follow-up. With neither `XDG_SESSION_TYPE`
    /// nor `WAYLAND_DISPLAY` set, an 8-byte zero-padded XID buffer
    /// must default to X11 (the older code defaulted to Wayland on
    /// `bytes.len() == 8`, which mis-routed legitimate X11 sessions).
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    #[serial]
    fn no_session_signals_defaults_to_x11_for_8_byte_buffer() {
        let prev_session = std::env::var("XDG_SESSION_TYPE").ok();
        let prev_display = std::env::var("WAYLAND_DISPLAY").ok();
        // SAFETY: process-global env mutation. See sibling tests for
        // the same rationale.
        unsafe {
            std::env::remove_var("XDG_SESSION_TYPE");
            std::env::remove_var("WAYLAND_DISPLAY");
        }
        let xid: u32 = 0x0042_4242;
        let mut bytes = [0u8; 8];
        bytes[..4].copy_from_slice(&xid.to_le_bytes());
        let handle = handle_from_bytes(&bytes).expect("x11 wrap with zero-padded buffer");
        assert_eq!(handle.platform(), NativePlatform::X11);
        // Restore env.
        unsafe {
            match prev_session {
                Some(v) => std::env::set_var("XDG_SESSION_TYPE", v),
                None => std::env::remove_var("XDG_SESSION_TYPE"),
            }
            match prev_display {
                Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
                None => std::env::remove_var("WAYLAND_DISPLAY"),
            }
        }
    }
}
