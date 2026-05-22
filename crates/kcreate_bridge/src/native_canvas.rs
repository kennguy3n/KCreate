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
    let nz = NonZeroIsize::new(raw)
        .ok_or(NativeCanvasError::NullHandle { platform: "windows" })?;
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
    // Linux can be either X11 or Wayland. We pick based on
    // XDG_SESSION_TYPE, defaulting to X11 (which is what Electron
    // returns by default on most distros). For X11 the handle bytes
    // are 4 bytes (the XID is a 32-bit unsigned int); for Wayland
    // they are 8 bytes (`wl_surface*`).
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    if session.eq_ignore_ascii_case("wayland") || bytes.len() == 8 {
        wayland_handle(bytes)
    } else {
        x11_handle(bytes)
    }
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
    use raw_window_handle::{WaylandDisplayHandle, WaylandWindowHandle};
    let ptr = read_ptr(bytes, "wayland")?;
    let nn = NonNull::new(ptr).ok_or(NativeCanvasError::NullHandle { platform: "wayland" })?;
    let window = RawWindowHandle::Wayland(WaylandWindowHandle::new(nn));
    let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(NonNull::dangling()));
    Ok(PlatformHandle {
        window,
        display,
        platform: NativePlatform::Wayland,
    })
}

#[cfg(not(any(target_os = "macos", target_os = "windows", unix)))]
fn interpret_bytes(_bytes: &[u8]) -> Result<PlatformHandle, NativeCanvasError> {
    Err(NativeCanvasError::UnsupportedPlatform)
}

#[allow(dead_code)]
fn read_ptr(bytes: &[u8], platform: &'static str) -> Result<*mut std::ffi::c_void, NativeCanvasError> {
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

#[cfg(test)]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
mod tests {
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
}
