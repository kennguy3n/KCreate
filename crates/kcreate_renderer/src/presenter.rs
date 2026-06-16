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

/// A rectangular sub-region of the framebuffer, in device pixels
/// (the same space the renderer rasterises into). Produced by
/// [`Presenter::take_present`] to tell the host exactly which pixels
/// changed since it last consumed a frame, so it can `putImageData`
/// only that sub-rect instead of repainting the whole canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirtyRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl DirtyRect {
    /// Pixel area of the rectangle. Widened to `u64` so a 4K-scale
    /// `width * height` can't overflow a `u32`.
    #[must_use]
    pub const fn area(&self) -> u64 {
        (self.width as u64) * (self.height as u64)
    }

    /// `true` when the rectangle covers no pixels.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// What changed in the framebuffer since the host last consumed a
/// frame. Accumulated across every publish so that a host whose
/// present loop skips intermediate frames still repaints every pixel
/// that changed while it was away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirtyAccum {
    /// No published frame differs from what the host last saw.
    Clean,
    /// A bounded rectangle changed.
    Region(DirtyRect),
    /// The whole frame must be treated as changed — the first frame,
    /// the frame after a resize, or a buffer-length mismatch. The
    /// full/partial *size* threshold is applied later in
    /// [`Presenter::take_present`], never here, so several small edits
    /// keep accumulating into a still-small rectangle.
    Full,
}

impl DirtyAccum {
    fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Full, _) | (_, Self::Full) => Self::Full,
            (Self::Clean, x) | (x, Self::Clean) => x,
            (Self::Region(a), Self::Region(b)) => Self::Region(union_rect(a, b)),
        }
    }
}

/// Bounding-box union of two pixel rectangles.
fn union_rect(a: DirtyRect, b: DirtyRect) -> DirtyRect {
    let min_x = a.x.min(b.x);
    let min_y = a.y.min(b.y);
    let max_x = (a.x + a.width).max(b.x + b.width);
    let max_y = (a.y + a.height).max(b.y + b.height);
    DirtyRect {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    }
}

/// Per-frame pixel diff: the tightest rectangle enclosing every pixel
/// that differs between two equally-sized, equally-dimensioned RGBA8
/// buffers.
///
/// Rows are compared with a single slice `==` (a vectorised `memcmp`),
/// so an unchanged row — the overwhelmingly common case for a typical
/// edit on a dense document — costs one fast compare and no per-pixel
/// work. Only rows that actually differ pay the inner first/last
/// differing-byte scan that tightens the horizontal bounds.
///
/// The result is exact: it is precisely the set of changed pixels, so
/// presenting only this rectangle can never drop a real change. It can
/// only ever be conservative in the trivial sense that it is a
/// bounding box (it may include unchanged pixels *between* two changed
/// islands on the same rows), which is always safe to repaint.
fn diff_dirty(old: &[u8], new: &[u8], width: u32, height: u32) -> DirtyAccum {
    let w = width as usize;
    let h = height as usize;
    let stride = w * 4;
    // Defensive: a length/dimension mismatch means we cannot trust a
    // pixel diff, so repaint everything.
    if stride == 0 || h == 0 || old.len() != stride * h || new.len() != stride * h {
        return DirtyAccum::Full;
    }

    let mut any = false;
    let mut min_x = w;
    let mut min_y = 0usize;
    let mut max_x = 0usize; // exclusive
    let mut max_y = 0usize; // exclusive

    for y in 0..h {
        let row = y * stride;
        let old_row = &old[row..row + stride];
        let new_row = &new[row..row + stride];
        if old_row == new_row {
            continue;
        }
        // The row differs, so there is at least one differing byte.
        let mut first = 0usize;
        while old_row[first] == new_row[first] {
            first += 1;
        }
        let mut last = stride;
        while old_row[last - 1] == new_row[last - 1] {
            last -= 1;
        }
        let first_px = first / 4;
        let last_px = last.div_ceil(4); // exclusive pixel index
        if first_px < min_x {
            min_x = first_px;
        }
        if last_px > max_x {
            max_x = last_px;
        }
        if !any {
            min_y = y;
            any = true;
        }
        max_y = y + 1;
    }

    if !any {
        return DirtyAccum::Clean;
    }
    DirtyAccum::Region(DirtyRect {
        x: min_x as u32,
        y: min_y as u32,
        width: (max_x - min_x) as u32,
        height: (max_y - min_y) as u32,
    })
}

/// A frame snapshot ready to ship to the host, carrying only the bytes
/// the host actually needs to repaint.
///
/// - `full == true`: `bytes` is the entire frame (`width*height*4`).
///   The host repaints with `putImageData(img, 0, 0)`.
/// - `full == false`: `bytes` is the `dirty` sub-rect, tightly packed
///   (`dirty.width*dirty.height*4`, and possibly empty when nothing
///   changed). The host patches its persistent backbuffer and repaints
///   with the dirty-rectangle form `putImageData(img, 0, 0, dx, dy,
///   dw, dh)`.
#[derive(Debug, Clone)]
pub struct PresentSnapshot {
    pub frame_id: FrameId,
    pub width: u32,
    pub height: u32,
    pub dirty: DirtyRect,
    pub full: bool,
    pub bytes: Vec<u8>,
}

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
    /// Union of every per-frame dirty rectangle published since the
    /// host last consumed a frame via [`Presenter::take_present`].
    /// Reset to [`DirtyAccum::Clean`] on each consume.
    dirty: DirtyAccum,
    /// Forces the next [`Presenter::publish_diff`] to report a full
    /// frame regardless of pixel content. Set on resize (the previous
    /// `published` bytes were `resize`d in place and no longer hold a
    /// coherent earlier frame to diff against). Consumed by the next
    /// diff.
    force_full_next: bool,
}

impl Presenter {
    pub fn new(width: u32, height: u32) -> Self {
        let inner = Inner {
            published: Slot::empty(width, height),
            idle: Slot::empty(width, height),
            width,
            height,
            has_published: false,
            dirty: DirtyAccum::Clean,
            force_full_next: false,
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
        // The resized `published` buffer no longer holds a coherent
        // previous frame to diff against, so the next publish must be a
        // full frame. Any region accumulated at the old dimensions is
        // meaningless now, so collapse the pending dirty state to a full
        // present too.
        g.force_full_next = true;
        g.dirty = DirtyAccum::Full;
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
                // A dimension change without a `resize` call likewise
                // invalidates the diff baseline and any region
                // accumulated at the old dimensions.
                g.force_full_next = true;
                g.dirty = DirtyAccum::Full;
            }
            std::mem::take(&mut g.idle.bytes)
        };
        let needed = (width as usize) * (height as usize) * 4;
        buf.clear();
        buf.reserve(needed);
        buf
    }

    /// Publish the staging buffer as the new latest frame, marking the
    /// whole frame dirty. Retained for callers (and tests) that don't
    /// need the per-frame pixel diff; the diff-tracking present path
    /// uses [`Self::publish_diff`].
    pub fn publish(&self, id: FrameId, bytes: Vec<u8>) {
        let mut g = self.inner.write();
        g.force_full_next = false;
        let old_published = std::mem::replace(&mut g.published, Slot { id, bytes });
        g.idle = old_published;
        g.has_published = true;
        g.dirty = g.dirty.union(DirtyAccum::Full);
    }

    /// Publish the staging buffer as the new latest frame, computing the
    /// exact rectangle of pixels that changed versus the currently
    /// published frame and folding it into the accumulated dirty region.
    ///
    /// The diff is what makes dirty-rect presentation correct *and*
    /// cheap: the renderer always re-rasterises the whole frame (the CPU
    /// backend clears and redraws every visible object), so the only
    /// reliable source of "what actually changed on screen" is the
    /// pixels themselves. Comparing the freshly rasterised frame N
    /// against the still-published frame N-1 yields precisely the changed
    /// pixels, so presenting only that rectangle can never drop a real
    /// change.
    pub fn publish_diff(&self, id: FrameId, bytes: Vec<u8>) {
        let mut g = self.inner.write();
        let frame_dirty = if !g.has_published || g.force_full_next {
            DirtyAccum::Full
        } else {
            diff_dirty(&g.published.bytes, &bytes, g.width, g.height)
        };
        g.force_full_next = false;
        let old_published = std::mem::replace(&mut g.published, Slot { id, bytes });
        g.idle = old_published;
        g.has_published = true;
        g.dirty = g.dirty.union(frame_dirty);
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

    /// Snapshot the latest published frame for presentation and reset
    /// the accumulated dirty region.
    ///
    /// Returns `None` only when no frame has ever been published.
    /// Otherwise the returned [`PresentSnapshot`] carries the minimum
    /// bytes the host needs to repaint:
    ///
    /// - **Nothing changed** since the last consume → an empty
    ///   ([`DirtyRect::is_empty`]) partial snapshot with no bytes. The
    ///   host keeps showing what it already has.
    /// - **A bounded region changed** covering less than
    ///   `max_partial_fraction` of the frame → a partial snapshot whose
    ///   `bytes` are just that sub-rect, tightly packed.
    /// - **A large region changed** (≥ `max_partial_fraction`), the
    ///   first frame, or a post-resize frame → a full snapshot carrying
    ///   the whole frame's bytes.
    ///
    /// The partial gather copies only the changed rows out of the
    /// published buffer, so the per-present copy cost scales with the
    /// edit, not with the framebuffer.
    pub fn take_present(&self, max_partial_fraction: f32) -> Option<PresentSnapshot> {
        let mut g = self.inner.write();
        if !g.has_published {
            return None;
        }
        let dirty = std::mem::replace(&mut g.dirty, DirtyAccum::Clean);
        let width = g.width;
        let height = g.height;
        let frame_id = g.published.id;

        let region = match dirty {
            DirtyAccum::Clean => {
                return Some(PresentSnapshot {
                    frame_id,
                    width,
                    height,
                    dirty: DirtyRect {
                        x: 0,
                        y: 0,
                        width: 0,
                        height: 0,
                    },
                    full: false,
                    bytes: Vec::new(),
                });
            }
            DirtyAccum::Full => None,
            DirtyAccum::Region(r) => {
                let frame_area = u64::from(width) * u64::from(height);
                let threshold =
                    (frame_area as f64 * f64::from(max_partial_fraction.clamp(0.0, 1.0))) as u64;
                // Defensive: a region accumulated before a resize can
                // outlive the buffer it described. If it no longer fits
                // the current frame (or the published buffer was
                // reallocated to new dimensions), present the whole frame
                // rather than gathering out-of-bounds rows.
                let fits = u64::from(r.x) + u64::from(r.width) <= u64::from(width)
                    && u64::from(r.y) + u64::from(r.height) <= u64::from(height)
                    && g.published.bytes.len() == width as usize * height as usize * 4;
                if !fits || r.area() >= threshold {
                    None
                } else {
                    Some(r)
                }
            }
        };

        match region {
            // Full-frame present: copy the whole published buffer (same
            // cost as the legacy `acquire_frame` path).
            None => Some(PresentSnapshot {
                frame_id,
                width,
                height,
                dirty: DirtyRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
                full: true,
                bytes: g.published.bytes.clone(),
            }),
            // Partial present: gather only the dirty rows, tightly packed
            // into a `dirty.width * dirty.height * 4` buffer.
            Some(r) => {
                let stride = width as usize * 4;
                let row_bytes = r.width as usize * 4;
                let src = &g.published.bytes;
                let mut bytes = Vec::with_capacity(row_bytes * r.height as usize);
                for row in 0..r.height as usize {
                    let y = r.y as usize + row;
                    let start = y * stride + r.x as usize * 4;
                    bytes.extend_from_slice(&src[start..start + row_bytes]);
                }
                Some(PresentSnapshot {
                    frame_id,
                    width,
                    height,
                    dirty: r,
                    full: false,
                    bytes,
                })
            }
        }
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

    // --- Dirty-rect present path ---------------------------------------

    /// A `w*h` RGBA8 frame filled with a single byte value.
    fn solid_frame(w: u32, h: u32, v: u8) -> Vec<u8> {
        vec![v; (w as usize) * (h as usize) * 4]
    }

    /// Set one pixel (x, y) in an RGBA8 `w`-wide buffer.
    fn set_px(buf: &mut [u8], w: u32, x: u32, y: u32, rgba: [u8; 4]) {
        let i = ((y * w + x) as usize) * 4;
        buf[i..i + 4].copy_from_slice(&rgba);
    }

    #[test]
    fn first_present_is_full_frame() {
        let p = Presenter::new(8, 8);
        p.publish_diff(FrameId(1), solid_frame(8, 8, 0x20));
        let snap = p.take_present(0.5).expect("present");
        assert!(snap.full, "first frame must be a full present");
        assert_eq!(snap.frame_id, FrameId(1));
        assert_eq!(
            snap.dirty,
            DirtyRect {
                x: 0,
                y: 0,
                width: 8,
                height: 8
            }
        );
        assert_eq!(snap.bytes.len(), 8 * 8 * 4);
    }

    #[test]
    fn unchanged_frame_presents_nothing() {
        let p = Presenter::new(8, 8);
        p.publish_diff(FrameId(1), solid_frame(8, 8, 0x20));
        // Consume the initial full frame.
        let _ = p.take_present(0.5).expect("first present");
        // Republish identical pixels — nothing changed.
        p.publish_diff(FrameId(2), solid_frame(8, 8, 0x20));
        let snap = p.take_present(0.5).expect("present");
        assert!(!snap.full);
        assert!(
            snap.dirty.is_empty(),
            "no pixels changed → empty dirty rect"
        );
        assert!(snap.bytes.is_empty());
        assert_eq!(snap.frame_id, FrameId(2));
    }

    #[test]
    fn single_pixel_edit_yields_one_pixel_partial() {
        let p = Presenter::new(16, 16);
        p.publish_diff(FrameId(1), solid_frame(16, 16, 0x00));
        let _ = p.take_present(0.5).expect("first present");

        let mut next = solid_frame(16, 16, 0x00);
        set_px(&mut next, 16, 5, 9, [1, 2, 3, 4]);
        p.publish_diff(FrameId(2), next);

        let snap = p.take_present(0.5).expect("present");
        assert!(!snap.full);
        assert_eq!(
            snap.dirty,
            DirtyRect {
                x: 5,
                y: 9,
                width: 1,
                height: 1
            }
        );
        assert_eq!(snap.bytes, vec![1, 2, 3, 4]);
    }

    #[test]
    fn partial_bytes_match_published_subrect() {
        let p = Presenter::new(32, 32);
        p.publish_diff(FrameId(1), solid_frame(32, 32, 0x10));
        let _ = p.take_present(0.5).expect("first present");

        // Change a 4x3 block at (10, 6).
        let mut next = solid_frame(32, 32, 0x10);
        for y in 6..9 {
            for x in 10..14 {
                set_px(&mut next, 32, x, y, [x as u8, y as u8, 0x55, 0xff]);
            }
        }
        p.publish_diff(FrameId(2), next.clone());

        let snap = p.take_present(0.5).expect("present");
        assert!(!snap.full);
        assert_eq!(
            snap.dirty,
            DirtyRect {
                x: 10,
                y: 6,
                width: 4,
                height: 3
            }
        );
        // The gathered bytes must be exactly the published sub-rect rows,
        // tightly packed.
        let mut expected = Vec::new();
        for y in 6..9usize {
            let start = (y * 32 + 10) * 4;
            expected.extend_from_slice(&next[start..start + 4 * 4]);
        }
        assert_eq!(snap.bytes, expected);
    }

    #[test]
    fn large_change_falls_back_to_full_frame() {
        let p = Presenter::new(16, 16);
        p.publish_diff(FrameId(1), solid_frame(16, 16, 0x00));
        let _ = p.take_present(0.5).expect("first present");

        // Repaint the whole frame a different colour: 100% > 50% → full.
        p.publish_diff(FrameId(2), solid_frame(16, 16, 0xAA));
        let snap = p.take_present(0.5).expect("present");
        assert!(
            snap.full,
            "a full-frame change must present the whole frame"
        );
        assert_eq!(snap.bytes.len(), 16 * 16 * 4);
    }

    #[test]
    fn dirty_accumulates_across_unconsumed_frames() {
        // The host's present loop may skip intermediate frames; the
        // accumulated dirty region must still cover every pixel that
        // changed since the host last consumed.
        let p = Presenter::new(64, 64);
        p.publish_diff(FrameId(1), solid_frame(64, 64, 0x00));
        let _ = p.take_present(0.5).expect("first present");

        let mut a = solid_frame(64, 64, 0x00);
        set_px(&mut a, 64, 2, 2, [1, 1, 1, 1]);
        p.publish_diff(FrameId(2), a);

        // Second edit, far away, WITHOUT consuming the first.
        let mut b = solid_frame(64, 64, 0x00);
        set_px(&mut b, 64, 2, 2, [1, 1, 1, 1]); // keep the first edit
        set_px(&mut b, 64, 40, 30, [2, 2, 2, 2]);
        p.publish_diff(FrameId(3), b);

        let snap = p.take_present(0.5).expect("present");
        assert!(!snap.full);
        // Bounding box of (2,2) and (40,30): x 2..41, y 2..31.
        assert_eq!(
            snap.dirty,
            DirtyRect {
                x: 2,
                y: 2,
                width: 39,
                height: 29
            }
        );
        assert_eq!(snap.frame_id, FrameId(3));
    }

    #[test]
    fn resize_forces_a_full_present() {
        let p = Presenter::new(8, 8);
        p.publish_diff(FrameId(1), solid_frame(8, 8, 0x33));
        let _ = p.take_present(0.5).expect("first present");

        // Resize keeps the dimensions' product but invalidates the diff
        // baseline; even republishing identical-looking content must be
        // a full present.
        p.resize(8, 8);
        p.publish_diff(FrameId(2), solid_frame(8, 8, 0x33));
        let snap = p.take_present(0.5).expect("present");
        assert!(snap.full, "post-resize frame must be a full present");
    }

    #[test]
    fn take_present_before_any_publish_is_none() {
        let p = Presenter::new(8, 8);
        assert!(p.take_present(0.5).is_none());
    }

    #[test]
    fn resize_discards_unconsumed_partial_region() {
        // A small edit accumulates a partial region that the host hasn't
        // consumed yet. A resize then changes the framebuffer out from
        // under it — the next present must be full (not a stale sub-rect
        // gathered from the reallocated buffer).
        let p = Presenter::new(16, 16);
        p.publish_diff(FrameId(1), solid_frame(16, 16, 0x00));
        let _ = p.take_present(0.5).expect("first present");

        let mut edited = solid_frame(16, 16, 0x00);
        set_px(&mut edited, 16, 3, 3, [1, 2, 3, 4]);
        p.publish_diff(FrameId(2), edited);
        // Do NOT consume; resize while a partial region is pending.
        p.resize(32, 32);
        p.publish_diff(FrameId(3), solid_frame(32, 32, 0x00));
        let snap = p.take_present(0.5).expect("present");
        assert!(snap.full, "post-resize present must be full");
        assert_eq!(snap.width, 32);
        assert_eq!(snap.height, 32);
        assert_eq!(snap.bytes.len(), 32 * 32 * 4);
    }
}
