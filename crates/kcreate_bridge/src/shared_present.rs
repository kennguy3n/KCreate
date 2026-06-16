//! Shared-memory framebuffer handoff — push past the dirty-rect IPC
//! ceiling toward locked 60fps.
//!
//! The legacy present path copies the changed pixels of every frame
//! across the Electron main→renderer IPC boundary (a structured-clone
//! of a `Buffer`). For a *full-frame* change — pan, zoom, scroll on a
//! dense document — that payload is the whole framebuffer
//! (`width*height*4`, e.g. ~7.9 MiB at 1920×1080) and it moves over IPC
//! **every frame**. That serialization + cross-process copy + async
//! round-trip is the ceiling this module removes.
//!
//! ## How it works
//!
//! A small file (under `/dev/shm` on Linux, the OS temp dir elsewhere)
//! is `mmap`'d by **both** processes:
//!
//! - The Electron **main** process (where the renderer lives) owns a
//!   [`SharedFramePublisher`]. After each offscreen render it copies the
//!   freshly published framebuffer into the next slot of a small ring
//!   under a **seqlock** and bumps a shared "latest" pointer.
//! - The Electron **renderer** process opens a [`SharedFrameReader`] over
//!   the same file (the tiny [`SharedPresentDescriptor`] — path + geometry
//!   — is handed over **once** via a normal IPC call) and, each animation
//!   frame, seqlock-reads the latest slot straight into its persistent
//!   `ImageData` backbuffer.
//!
//! After the one-time descriptor handshake **zero frame bytes cross
//! IPC**: the renderer reads the framebuffer directly out of shared
//! memory. The per-frame structured-clone is gone.
//!
//! ## Concurrency: a single-writer / many-reader seqlock
//!
//! The publisher is the only writer. Each slot carries a `seq` counter:
//! the writer makes it **odd** before touching the slot and **even**
//! (incremented) after, with release fences bracketing the body. A reader
//! samples `seq` before and after copying; if it saw an odd `seq` or the
//! two samples differ, the writer was mid-update and the reader retries.
//! A 3-slot ring means the writer has to lap the reader twice before it
//! could overwrite the slot a reader is actively copying, and the seqlock
//! catches even that. Tearing therefore degrades to a retry, never to a
//! torn frame reaching the canvas.
//!
//! All `unsafe` (the `mmap` calls) is encapsulated in
//! [`crate::native_canvas`]; this module is `unsafe`-free and operates
//! only on the safe `&[u8]` / `&mut [u8]` views that module hands out.

#![cfg(feature = "native_canvas")]

use std::fs::{File, OpenOptions};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{fence, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::native_canvas::{MappedRegion, MappedRegionRo};

/// Magic number at the head of the shared region: the bytes `KCSF`
/// (KCreate Shared Frame). Serialized little-endian via `write_u32`.
const MAGIC: u32 = 0x4B43_5346;
/// On-disk layout version. Bumped if the header / slot layout changes.
const VERSION: u32 = 1;
/// Fixed header length (bytes). A multiple of 4 so every slot — and
/// therefore every `seq` / pixel run — stays 4-byte aligned.
const HEADER_LEN: usize = 64;
/// Per-slot metadata length (bytes), preceding the slot's pixel run.
/// Also a multiple of 4.
const SLOT_META_LEN: usize = 32;
/// Sentinel "no frame published yet" value for the header `latest` field.
const NO_SLOT: u32 = u32::MAX;
/// Default ring depth. Three slots let the reader copy slot N while the
/// writer advances through N+1 and N+2 before it could revisit N.
pub const DEFAULT_SLOT_COUNT: u32 = 3;
/// Bound on seqlock read retries before the reader gives up for this
/// tick (the host then keeps showing the frame it already has).
const MAX_READ_RETRIES: u32 = 16;

// Header field byte offsets.
const H_MAGIC: usize = 0;
const H_VERSION: usize = 4;
const H_WIDTH: usize = 8;
const H_HEIGHT: usize = 12;
const H_SLOT_COUNT: usize = 16;
const H_SLOT_STRIDE: usize = 20;
const H_LATEST: usize = 24;
const H_PIXEL_LEN: usize = 28;

// Per-slot metadata field offsets (relative to the slot start).
const S_SEQ: usize = 0;
const S_FULL: usize = 4;
const S_FRAME_ID: usize = 8; // u64 (8 bytes)
const S_DIRTY_X: usize = 16;
const S_DIRTY_Y: usize = 20;
const S_DIRTY_W: usize = 24;
const S_DIRTY_H: usize = 28;

/// Geometry + path needed by the renderer process to map the same
/// shared region the publisher created. Exchanged once over IPC; after
/// that no frame bytes cross the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedPresentDescriptor {
    /// Absolute path of the mmap-backed file.
    pub path: String,
    /// Total mapped length in bytes (header + all slots).
    pub len: u64,
    /// Framebuffer width in physical pixels.
    pub width: u32,
    /// Framebuffer height in physical pixels.
    pub height: u32,
    /// Ring depth.
    pub slot_count: u32,
}

/// Metadata describing the frame a reader just copied out of a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedFrameMeta {
    pub frame_id: u64,
    pub width: u32,
    pub height: u32,
    pub dirty_x: u32,
    pub dirty_y: u32,
    pub dirty_width: u32,
    pub dirty_height: u32,
    pub full: bool,
}

/// Errors from the shared-memory present path.
#[derive(Debug, thiserror::Error)]
pub enum SharedPresentError {
    #[error("shared-present I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("shared-present region too small: mapped {got} bytes, need at least {need}")]
    RegionTooSmall { got: usize, need: usize },
    #[error("shared-present header invalid (magic {magic:#010x}, version {version})")]
    BadHeader { magic: u32, version: u32 },
    #[error("shared-present geometry mismatch: descriptor {descriptor:?}, header {header:?}")]
    GeometryMismatch {
        descriptor: (u32, u32, u32),
        header: (u32, u32, u32),
    },
    #[error("invalid shared-present dimensions {width}x{height}")]
    BadDimensions { width: u32, height: u32 },
}

fn read_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(buf[off..off + 4].try_into().expect("4-byte slice"))
}

fn write_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn read_u64(buf: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(buf[off..off + 8].try_into().expect("8-byte slice"))
}

fn write_u64(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

/// Directory for the shared region. Prefers Linux `tmpfs` (`/dev/shm`)
/// so the file is genuinely RAM-backed; falls back to the OS temp dir.
fn shared_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        let shm = std::path::Path::new("/dev/shm");
        if shm.is_dir() {
            return shm.to_path_buf();
        }
    }
    std::env::temp_dir()
}

/// Process-unique file name. The pid plus a monotonic counter plus a
/// nanosecond stamp keeps concurrent publishers (e.g. tests) from
/// colliding without pulling in a tempfile/rng dependency.
fn unique_path(width: u32, height: u32) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    shared_dir().join(format!(
        "kcreate-present-{}-{}-{}-{}x{}.bin",
        std::process::id(),
        n,
        nanos,
        width,
        height
    ))
}

fn pixel_len(width: u32, height: u32) -> usize {
    width as usize * height as usize * 4
}

fn region_len(width: u32, height: u32, slot_count: u32) -> usize {
    HEADER_LEN + slot_stride(width, height) * slot_count as usize
}

fn slot_stride(width: u32, height: u32) -> usize {
    SLOT_META_LEN + pixel_len(width, height)
}

/// Publisher side of the shared-memory ring (Electron main process).
#[derive(Debug)]
pub struct SharedFramePublisher {
    region: MappedRegion,
    // Kept alive alongside the mapping; see the safety contract in
    // `native_canvas`.
    _file: File,
    path: PathBuf,
    width: u32,
    height: u32,
    slot_count: u32,
    slot_stride: usize,
    pixel_len: usize,
    next_slot: u32,
}

impl SharedFramePublisher {
    /// Create and map a fresh shared region sized for `width`×`height`
    /// frames with `slot_count` ring slots.
    pub fn create(
        width: u32,
        height: u32,
        slot_count: u32,
    ) -> std::result::Result<Self, SharedPresentError> {
        if width == 0 || height == 0 {
            return Err(SharedPresentError::BadDimensions { width, height });
        }
        let slot_count = slot_count.max(2);
        let stride = slot_stride(width, height);
        let pixels = pixel_len(width, height);
        let total = region_len(width, height, slot_count);
        let path = unique_path(width, height);

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.set_len(total as u64)?;
        let mut region = MappedRegion::create(&file, total)?;

        {
            let buf = region.as_mut_slice();
            // Zero the header (the OS already zeroes a freshly `set_len`'d
            // file, but be explicit so a reused inode can't surprise us).
            for b in &mut buf[..HEADER_LEN] {
                *b = 0;
            }
            write_u32(buf, H_MAGIC, MAGIC);
            write_u32(buf, H_VERSION, VERSION);
            write_u32(buf, H_WIDTH, width);
            write_u32(buf, H_HEIGHT, height);
            write_u32(buf, H_SLOT_COUNT, slot_count);
            write_u32(buf, H_SLOT_STRIDE, stride as u32);
            write_u32(buf, H_LATEST, NO_SLOT);
            write_u32(buf, H_PIXEL_LEN, pixels as u32);
        }

        Ok(Self {
            region,
            _file: file,
            path,
            width,
            height,
            slot_count,
            slot_stride: stride,
            pixel_len: pixels,
            next_slot: 0,
        })
    }

    /// Descriptor the renderer process needs to map the same region.
    #[must_use]
    pub fn descriptor(&self) -> SharedPresentDescriptor {
        SharedPresentDescriptor {
            path: self.path.to_string_lossy().into_owned(),
            len: region_len(self.width, self.height, self.slot_count) as u64,
            width: self.width,
            height: self.height,
            slot_count: self.slot_count,
        }
    }

    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Publish a full framebuffer into the next ring slot.
    ///
    /// `pixels` must be exactly `width*height*4` bytes (the publisher's
    /// configured geometry). A mismatch — e.g. a frame produced after a
    /// resize but before the publisher was re-created — is rejected so a
    /// reader never copies an out-of-geometry slot; the host re-handshakes
    /// on resize. Returns `true` when the frame was published.
    pub fn publish_full(&mut self, frame_id: u64, pixels: &[u8]) -> bool {
        let (w, h) = (self.width, self.height);
        self.publish(frame_id, true, (0, 0, w, h), pixels)
    }

    /// Publish a frame, marking the changed sub-rect. `full` flags whether
    /// `dirty` spans the whole frame. The whole framebuffer is always
    /// copied into the slot (shared memory makes a full copy free of IPC);
    /// `dirty` is carried so a future reader could blit just the sub-rect.
    pub fn publish(
        &mut self,
        frame_id: u64,
        full: bool,
        dirty: (u32, u32, u32, u32),
        pixels: &[u8],
    ) -> bool {
        if pixels.len() != self.pixel_len {
            return false;
        }
        let slot = self.next_slot;
        let slot_off = HEADER_LEN + slot as usize * self.slot_stride;
        let pixels_off = slot_off + SLOT_META_LEN;
        let pixel_len = self.pixel_len;
        let buf = self.region.as_mut_slice();

        // Begin write: make the slot's seq odd so a concurrent reader
        // knows the slot is in flux and retries.
        let seq = read_u32(buf, slot_off + S_SEQ);
        let odd = seq | 1;
        write_u32(buf, slot_off + S_SEQ, odd);
        fence(Ordering::Release);

        // Slot metadata + pixels.
        write_u32(buf, slot_off + S_FULL, u32::from(full));
        write_u64(buf, slot_off + S_FRAME_ID, frame_id);
        write_u32(buf, slot_off + S_DIRTY_X, dirty.0);
        write_u32(buf, slot_off + S_DIRTY_Y, dirty.1);
        write_u32(buf, slot_off + S_DIRTY_W, dirty.2);
        write_u32(buf, slot_off + S_DIRTY_H, dirty.3);
        buf[pixels_off..pixels_off + pixel_len].copy_from_slice(pixels);

        // End write: bump seq back to an (incremented) even value, then
        // publish this slot as the latest. The release fences keep the
        // metadata/pixel writes from sinking past the seq/latest stores.
        fence(Ordering::Release);
        write_u32(buf, slot_off + S_SEQ, odd.wrapping_add(1));
        fence(Ordering::Release);
        write_u32(buf, H_LATEST, slot);

        self.next_slot = (slot + 1) % self.slot_count;
        true
    }
}

impl Drop for SharedFramePublisher {
    fn drop(&mut self) {
        // Best-effort unlink. On Unix the inode (and any reader's live
        // mapping) survives until the last reference is dropped, so this
        // never pulls the rug out from under a reader mid-frame.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Reader side of the shared-memory ring (Electron renderer process).
#[derive(Debug)]
pub struct SharedFrameReader {
    region: MappedRegionRo,
    _file: File,
    width: u32,
    height: u32,
    slot_count: u32,
    slot_stride: usize,
    pixel_len: usize,
    last_frame_id: Option<u64>,
}

impl SharedFrameReader {
    /// Open and map the region described by `descriptor` read-only.
    pub fn open(
        descriptor: &SharedPresentDescriptor,
    ) -> std::result::Result<Self, SharedPresentError> {
        if descriptor.width == 0 || descriptor.height == 0 {
            return Err(SharedPresentError::BadDimensions {
                width: descriptor.width,
                height: descriptor.height,
            });
        }
        let len = descriptor.len as usize;
        let need = region_len(
            descriptor.width,
            descriptor.height,
            descriptor.slot_count.max(2),
        );
        if len < need {
            return Err(SharedPresentError::RegionTooSmall { got: len, need });
        }
        let file = OpenOptions::new().read(true).open(&descriptor.path)?;
        let region = MappedRegionRo::open(&file, len)?;

        let buf = region.as_slice();
        if buf.len() < HEADER_LEN {
            return Err(SharedPresentError::RegionTooSmall {
                got: buf.len(),
                need: HEADER_LEN,
            });
        }
        let magic = read_u32(buf, H_MAGIC);
        let version = read_u32(buf, H_VERSION);
        if magic != MAGIC || version != VERSION {
            return Err(SharedPresentError::BadHeader { magic, version });
        }
        let width = read_u32(buf, H_WIDTH);
        let height = read_u32(buf, H_HEIGHT);
        let slot_count = read_u32(buf, H_SLOT_COUNT);
        let slot_stride = read_u32(buf, H_SLOT_STRIDE) as usize;
        if (width, height, slot_count)
            != (descriptor.width, descriptor.height, descriptor.slot_count)
        {
            return Err(SharedPresentError::GeometryMismatch {
                descriptor: (descriptor.width, descriptor.height, descriptor.slot_count),
                header: (width, height, slot_count),
            });
        }

        Ok(Self {
            region,
            _file: file,
            width,
            height,
            slot_count,
            slot_stride,
            pixel_len: pixel_len(width, height),
            last_frame_id: None,
        })
    }

    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    #[must_use]
    pub const fn frame_len(&self) -> usize {
        self.pixel_len
    }

    /// Copy the latest published frame into `dest` (which must be at
    /// least `width*height*4` bytes), returning its metadata.
    ///
    /// Returns `Ok(None)` when no frame has been published yet, or when
    /// the seqlock could not settle within [`MAX_READ_RETRIES`] (a writer
    /// that is pathologically hot — the host keeps its current frame).
    pub fn read_latest_into(
        &self,
        dest: &mut [u8],
    ) -> std::result::Result<Option<SharedFrameMeta>, SharedPresentError> {
        if dest.len() < self.pixel_len {
            return Err(SharedPresentError::RegionTooSmall {
                got: dest.len(),
                need: self.pixel_len,
            });
        }
        let buf = self.region.as_slice();
        for _ in 0..MAX_READ_RETRIES {
            let latest = read_u32(buf, H_LATEST);
            fence(Ordering::Acquire);
            if latest == NO_SLOT || latest >= self.slot_count {
                return Ok(None);
            }
            let slot_off = HEADER_LEN + latest as usize * self.slot_stride;
            let pixels_off = slot_off + SLOT_META_LEN;

            let seq1 = read_u32(buf, slot_off + S_SEQ);
            if seq1 & 1 != 0 {
                // Writer is mid-update on this slot; retry.
                continue;
            }
            fence(Ordering::Acquire);

            let full = read_u32(buf, slot_off + S_FULL) != 0;
            let frame_id = read_u64(buf, slot_off + S_FRAME_ID);
            let dirty_x = read_u32(buf, slot_off + S_DIRTY_X);
            let dirty_y = read_u32(buf, slot_off + S_DIRTY_Y);
            let dirty_width = read_u32(buf, slot_off + S_DIRTY_W);
            let dirty_height = read_u32(buf, slot_off + S_DIRTY_H);
            dest[..self.pixel_len].copy_from_slice(&buf[pixels_off..pixels_off + self.pixel_len]);

            fence(Ordering::Acquire);
            let seq2 = read_u32(buf, slot_off + S_SEQ);
            if seq1 == seq2 {
                return Ok(Some(SharedFrameMeta {
                    frame_id,
                    width: self.width,
                    height: self.height,
                    dirty_x,
                    dirty_y,
                    dirty_width,
                    dirty_height,
                    full,
                }));
            }
            // The writer overwrote this slot while we copied; retry.
        }
        Ok(None)
    }

    /// Like [`read_latest_into`] but returns `Ok(None)` when the latest
    /// frame id matches `since` (nothing new to present). Lets the host
    /// skip the copy + canvas blit when the frame hasn't advanced.
    ///
    /// [`read_latest_into`]: Self::read_latest_into
    pub fn read_new_into(
        &mut self,
        since: Option<u64>,
        dest: &mut [u8],
    ) -> std::result::Result<Option<SharedFrameMeta>, SharedPresentError> {
        let buf = self.region.as_slice();
        let latest = read_u32(buf, H_LATEST);
        if latest == NO_SLOT || latest >= self.slot_count {
            return Ok(None);
        }
        // Peek the latest slot's frame id without copying pixels.
        let slot_off = HEADER_LEN + latest as usize * self.slot_stride;
        let peek_id = read_u64(buf, slot_off + S_FRAME_ID);
        if since == Some(peek_id) && self.last_frame_id == Some(peek_id) {
            return Ok(None);
        }
        let meta = self.read_latest_into(dest)?;
        if let Some(m) = &meta {
            self.last_frame_id = Some(m.frame_id);
        }
        Ok(meta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkerboard(width: u32, height: u32, tick: u8) -> Vec<u8> {
        let mut v = vec![0u8; pixel_len(width, height)];
        for (i, px) in v.chunks_exact_mut(4).enumerate() {
            let on = (i + tick as usize).is_multiple_of(2);
            let c = if on { 0xFF } else { 0x10 };
            px[0] = c;
            px[1] = c.wrapping_add(tick);
            px[2] = c;
            px[3] = 0xFF;
        }
        v
    }

    #[test]
    fn publish_then_read_round_trips_full_frame() {
        let (w, h) = (16u32, 8u32);
        let mut pubr = SharedFramePublisher::create(w, h, DEFAULT_SLOT_COUNT).expect("create");
        let desc = pubr.descriptor();
        assert_eq!(desc.width, w);
        assert_eq!(desc.height, h);
        assert_eq!(desc.len as usize, region_len(w, h, DEFAULT_SLOT_COUNT));

        let reader = SharedFrameReader::open(&desc).expect("open");
        let mut dest = vec![0u8; pixel_len(w, h)];

        // Nothing published yet.
        assert!(reader.read_latest_into(&mut dest).expect("read").is_none());

        let frame = checkerboard(w, h, 1);
        assert!(pubr.publish_full(42, &frame));
        let meta = reader
            .read_latest_into(&mut dest)
            .expect("read")
            .expect("frame");
        assert_eq!(meta.frame_id, 42);
        assert!(meta.full);
        assert_eq!(meta.width, w);
        assert_eq!(meta.height, h);
        assert_eq!(
            dest, frame,
            "shared-memory frame must match published bytes"
        );
    }

    #[test]
    fn ring_wraps_and_reader_tracks_latest() {
        let (w, h) = (8u32, 8u32);
        let mut pubr = SharedFramePublisher::create(w, h, DEFAULT_SLOT_COUNT).expect("create");
        let reader = SharedFrameReader::open(&pubr.descriptor()).expect("open");
        let mut dest = vec![0u8; pixel_len(w, h)];

        // Publish more frames than there are slots so the ring wraps.
        for tick in 0..(DEFAULT_SLOT_COUNT * 3) {
            let frame = checkerboard(w, h, tick as u8);
            assert!(pubr.publish_full(u64::from(tick), &frame));
            let meta = reader
                .read_latest_into(&mut dest)
                .expect("read")
                .expect("frame");
            assert_eq!(meta.frame_id, u64::from(tick));
            assert_eq!(dest, frame);
        }
    }

    #[test]
    fn read_new_into_skips_unchanged_frames() {
        let (w, h) = (8u32, 8u32);
        let mut pubr = SharedFramePublisher::create(w, h, DEFAULT_SLOT_COUNT).expect("create");
        let mut reader = SharedFrameReader::open(&pubr.descriptor()).expect("open");
        let mut dest = vec![0u8; pixel_len(w, h)];

        let frame = checkerboard(w, h, 7);
        assert!(pubr.publish_full(100, &frame));
        let first = reader
            .read_new_into(None, &mut dest)
            .expect("read")
            .expect("frame");
        assert_eq!(first.frame_id, 100);
        // Same latest frame id → nothing new.
        assert!(reader
            .read_new_into(Some(100), &mut dest)
            .expect("read")
            .is_none());
    }

    #[test]
    fn publish_rejects_wrong_pixel_length() {
        let mut pubr = SharedFramePublisher::create(8, 8, DEFAULT_SLOT_COUNT).expect("create");
        assert!(!pubr.publish_full(1, &[0u8; 8 * 8 * 4 - 4]));
    }

    #[test]
    fn open_rejects_geometry_mismatch() {
        let pubr = SharedFramePublisher::create(8, 8, DEFAULT_SLOT_COUNT).expect("create");
        let mut desc = pubr.descriptor();
        desc.width = 9; // lie about the geometry
        let err = SharedFrameReader::open(&desc).expect_err("must reject");
        assert!(matches!(
            err,
            SharedPresentError::RegionTooSmall { .. } | SharedPresentError::GeometryMismatch { .. }
        ));
    }

    #[test]
    fn zero_dimensions_rejected() {
        assert!(matches!(
            SharedFramePublisher::create(0, 8, DEFAULT_SLOT_COUNT),
            Err(SharedPresentError::BadDimensions { .. })
        ));
    }
}
