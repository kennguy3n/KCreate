//! Frame delivery: holds the most recently rendered pixel buffer and
//! hands borrows of it to the N-API bridge.
//!
//! The Presenter uses a triple-buffer scheme:
//!
//! - `published`: the buffer the host is reading from.
//! - `staging`: the buffer the renderer is currently writing into.
//! - `idle`: the buffer waiting to swap in.
//!
//! This avoids `clone`-per-frame, allows reads while a new frame is
//! being rendered, and keeps allocations stable across frames (the
//! buffers only grow on resize).

use parking_lot::{RwLock, RwLockReadGuard};

/// Sequence number assigned to each published frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameId(pub u64);

#[derive(Debug)]
struct Slot {
    id: FrameId,
    bytes: Vec<u8>,
}

impl Slot {
    fn empty(width: u32, height: u32) -> Self {
        Self {
            id: FrameId(0),
            bytes: vec![0u8; (width as usize) * (height as usize) * 4],
        }
    }
}

#[derive(Debug)]
pub struct Presenter {
    inner: RwLock<Inner>,
}

#[derive(Debug)]
struct Inner {
    published: Slot,
    idle: Slot,
    // The currently-being-written buffer is `mem::take`n into a local on
    // `acquire_staging` and returned to `idle` on `publish`.
    width: u32,
    height: u32,
    /// True once any frame has been published. Tracked explicitly so
    /// [`Self::latest`] doesn't have to scan the buffer to decide
    /// whether a frame is available.
    has_published: bool,
}

impl Presenter {
    pub fn new(width: u32, height: u32) -> Self {
        let inner = Inner {
            published: Slot::empty(width, height),
            idle: Slot::empty(width, height),
            width,
            height,
            has_published: false,
        };
        Self {
            inner: RwLock::new(inner),
        }
    }

    pub fn resize(&self, width: u32, height: u32) {
        let mut g = self.inner.write();
        g.width = width;
        g.height = height;
        let needed = (width as usize) * (height as usize) * 4;
        g.published.bytes.resize(needed, 0);
        g.idle.bytes.resize(needed, 0);
    }

    /// Acquire a buffer to render into. The renderer writes RGBA8 pixels
    /// into the returned Vec then calls [`Self::publish`] (or
    /// [`Self::recycle_staging`] if rendering failed).
    pub fn acquire_staging(&self, width: u32, height: u32) -> Vec<u8> {
        let mut buf = {
            let mut g = self.inner.write();
            if g.width != width || g.height != height {
                g.width = width;
                g.height = height;
            }
            std::mem::take(&mut g.idle.bytes)
        };
        let needed = (width as usize) * (height as usize) * 4;
        buf.clear();
        buf.reserve(needed);
        buf
    }

    /// Publish the staging buffer as the new latest frame.
    pub fn publish(&self, id: FrameId, bytes: Vec<u8>) {
        let mut g = self.inner.write();
        let old_published = std::mem::replace(&mut g.published, Slot { id, bytes });
        g.idle = old_published;
        g.has_published = true;
    }

    /// Return a staging buffer to the idle pool without publishing it.
    /// Called when `render()` errors so the allocation is not lost.
    pub fn recycle_staging(&self, bytes: Vec<u8>) {
        let mut g = self.inner.write();
        // Keep whichever buffer has more capacity; drop the other to
        // bound memory growth across repeated render failures.
        if bytes.capacity() > g.idle.bytes.capacity() {
            g.idle.bytes = bytes;
        }
    }

    /// Borrow the latest published frame.
    pub fn latest(&self) -> Option<FrameLease<'_>> {
        let guard = self.inner.read();
        if !guard.has_published {
            return None;
        }
        Some(FrameLease { guard })
    }

    /// Borrow a specific previously-published frame if it is still the
    /// latest. (Older frames are not retained — this is a current-or-nothing
    /// lookup, matching the amendment's `get_frame_pixels` semantics.)
    pub fn lease(&self, frame: FrameId) -> Option<FrameLease<'_>> {
        let guard = self.inner.read();
        if guard.has_published && guard.published.id == frame {
            Some(FrameLease { guard })
        } else {
            None
        }
    }

    /// The dimensions of the current staging/published buffers.
    pub fn size(&self) -> (u32, u32) {
        let g = self.inner.read();
        (g.width, g.height)
    }
}

/// Read-only borrow of the latest frame's pixels.
pub struct FrameLease<'a> {
    guard: RwLockReadGuard<'a, Inner>,
}

impl FrameLease<'_> {
    pub fn pixels(&self) -> &[u8] {
        &self.guard.published.bytes
    }

    pub fn frame_id(&self) -> FrameId {
        self.guard.published.id
    }

    pub fn width(&self) -> u32 {
        self.guard.width
    }

    pub fn height(&self) -> u32 {
        self.guard.height
    }
}

impl std::fmt::Debug for FrameLease<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameLease")
            .field("id", &self.frame_id())
            .field("width", &self.width())
            .field("height", &self.height())
            .field("bytes_len", &self.pixels().len())
            .finish()
    }
}

#[cfg(test)]
#[allow(clippy::significant_drop_tightening)]
mod tests {
    use super::*;

    #[test]
    fn publish_then_lease_round_trips() {
        let p = Presenter::new(8, 8);
        let mut buf = p.acquire_staging(8, 8);
        buf.extend((0..8 * 8 * 4).map(|i| i as u8));
        p.publish(FrameId(1), buf);
        let lease = p.lease(FrameId(1)).expect("lease");
        assert_eq!(lease.frame_id(), FrameId(1));
        assert_eq!(lease.pixels().len(), 8 * 8 * 4);
        assert_eq!(lease.pixels()[5], 5);
    }

    #[test]
    fn lease_of_stale_id_returns_none() {
        let p = Presenter::new(4, 4);
        let buf = p.acquire_staging(4, 4);
        p.publish(FrameId(2), buf);
        assert!(p.lease(FrameId(1)).is_none());
        assert!(p.lease(FrameId(2)).is_some());
    }

    #[test]
    fn resize_grows_buffers() {
        let p = Presenter::new(4, 4);
        p.resize(16, 8);
        let buf = p.acquire_staging(16, 8);
        // capacity should accommodate new size
        assert!(buf.capacity() >= 16 * 8 * 4);
    }

    #[test]
    fn latest_returns_none_before_any_publish() {
        let p = Presenter::new(4, 4);
        assert!(p.latest().is_none());
    }

    #[test]
    fn all_zero_first_frame_is_still_visible() {
        // Regression: previously `latest()` did a byte-scan and reported
        // "no frame" when the first published frame was all-zero. Now we
        // track `has_published` explicitly so a black frame is a valid
        // frame.
        let p = Presenter::new(4, 4);
        let mut buf = p.acquire_staging(4, 4);
        buf.resize(4 * 4 * 4, 0);
        p.publish(FrameId(1), buf);
        let lease = p.latest().expect("frame should be visible even if black");
        assert_eq!(lease.frame_id(), FrameId(1));
        assert!(lease.pixels().iter().all(|&b| b == 0));
    }

    #[test]
    fn recycle_staging_after_failure_preserves_capacity() {
        let p = Presenter::new(4, 4);
        let mut buf = p.acquire_staging(8, 8);
        buf.reserve(8 * 8 * 4);
        let cap = buf.capacity();
        assert!(cap >= 8 * 8 * 4);
        p.recycle_staging(buf);
        // Next acquire should reuse that capacity.
        let buf2 = p.acquire_staging(8, 8);
        assert!(buf2.capacity() >= cap);
    }
}
