//! Tile grid: partition an RGBA image into fixed-size square tiles
//! with dirty tracking.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Default tile size in pixels (a common GPU-texture-friendly value).
pub const DEFAULT_TILE_SIZE: u32 = 256;

/// Errors from [`TileGrid`].
#[derive(Debug, Error)]
pub enum TileGridError {
    #[error("invalid dimensions: {width}x{height}")]
    InvalidDimensions { width: u32, height: u32 },
    #[error("invalid tile size: {0}")]
    InvalidTileSize(u32),
    #[error(
        "pixel buffer length {got} does not match expected {expected} for {width}x{height} RGBA"
    )]
    PixelBufferSize {
        got: usize,
        expected: usize,
        width: u32,
        height: u32,
    },
    #[error("tile ({col},{row}) is out of bounds for grid {cols}x{rows}")]
    OutOfBounds {
        col: u32,
        row: u32,
        cols: u32,
        rows: u32,
    },
}

/// A single tile in a [`TileGrid`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    /// RGBA8 pixels, row-major, exactly `tile_size * tile_size * 4`
    /// bytes for interior tiles. Edge tiles store the same number of
    /// bytes — pixels outside the parent image bounds are zeroed.
    pub pixels: Vec<u8>,
    pub dirty: bool,
}

impl Tile {
    /// Allocate an empty (zeroed RGBA) tile at the given grid origin.
    #[must_use]
    pub fn empty(x: u32, y: u32, tile_size: u32) -> Self {
        let n = (tile_size as usize)
            .saturating_mul(tile_size as usize)
            .saturating_mul(4);
        Self {
            x,
            y,
            pixels: vec![0; n],
            dirty: false,
        }
    }
}

/// A grid of tiles spanning a `width × height` RGBA image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TileGrid {
    pub width: u32,
    pub height: u32,
    pub tile_size: u32,
    cols: u32,
    rows: u32,
    /// Row-major (rows × cols) optional tiles. `None` means the tile
    /// is fully transparent and has not been allocated yet — this
    /// keeps memory low for sparsely-used canvases.
    tiles: Vec<Option<Tile>>,
}

impl TileGrid {
    /// Allocate an empty grid.
    pub fn new(width: u32, height: u32, tile_size: u32) -> Result<Self, TileGridError> {
        if width == 0 || height == 0 {
            return Err(TileGridError::InvalidDimensions { width, height });
        }
        if tile_size == 0 {
            return Err(TileGridError::InvalidTileSize(tile_size));
        }
        let cols = width.div_ceil(tile_size);
        let rows = height.div_ceil(tile_size);
        let len = (cols as usize).saturating_mul(rows as usize);
        Ok(Self {
            width,
            height,
            tile_size,
            cols,
            rows,
            tiles: vec![None; len],
        })
    }

    /// Number of tile columns.
    #[must_use]
    pub const fn cols(&self) -> u32 {
        self.cols
    }
    /// Number of tile rows.
    #[must_use]
    pub const fn rows(&self) -> u32 {
        self.rows
    }

    const fn index(&self, col: u32, row: u32) -> Result<usize, TileGridError> {
        if col >= self.cols || row >= self.rows {
            return Err(TileGridError::OutOfBounds {
                col,
                row,
                cols: self.cols,
                rows: self.rows,
            });
        }
        Ok((row as usize) * (self.cols as usize) + (col as usize))
    }

    /// Read access to a tile (if allocated).
    pub fn get_tile(&self, col: u32, row: u32) -> Result<Option<&Tile>, TileGridError> {
        let idx = self.index(col, row)?;
        Ok(self.tiles[idx].as_ref())
    }

    /// Mutable access, lazily allocating the tile.
    pub fn get_tile_mut(&mut self, col: u32, row: u32) -> Result<&mut Tile, TileGridError> {
        let idx = self.index(col, row)?;
        let x = col * self.tile_size;
        let y = row * self.tile_size;
        let ts = self.tile_size;
        let slot = &mut self.tiles[idx];
        if slot.is_none() {
            *slot = Some(Tile::empty(x, y, ts));
        }
        Ok(slot.as_mut().expect("tile slot was just initialised above"))
    }

    /// Mark a tile dirty (allocating if necessary).
    pub fn mark_dirty(&mut self, col: u32, row: u32) -> Result<(), TileGridError> {
        let tile = self.get_tile_mut(col, row)?;
        tile.dirty = true;
        Ok(())
    }

    /// Return the `(col, row)` of every dirty tile.
    #[must_use]
    pub fn dirty_tiles(&self) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for row in 0..self.rows {
            for col in 0..self.cols {
                let idx = (row as usize) * (self.cols as usize) + (col as usize);
                if let Some(t) = &self.tiles[idx] {
                    if t.dirty {
                        out.push((col, row));
                    }
                }
            }
        }
        out
    }

    /// Clear all dirty flags.
    pub fn clear_dirty(&mut self) {
        for t in self.tiles.iter_mut().flatten() {
            t.dirty = false;
        }
    }

    /// Build a tile grid from a flat RGBA8 buffer.
    pub fn from_image(
        rgba: &[u8],
        width: u32,
        height: u32,
        tile_size: u32,
    ) -> Result<Self, TileGridError> {
        let expected = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        if rgba.len() != expected {
            return Err(TileGridError::PixelBufferSize {
                got: rgba.len(),
                expected,
                width,
                height,
            });
        }
        let mut grid = Self::new(width, height, tile_size)?;
        // Parallel fill: each tile reads a window of the source.
        let cols = grid.cols;
        let rows = grid.rows;
        let ts = grid.tile_size;
        let mut tiles: Vec<Option<Tile>> = (0..(rows * cols) as usize).map(|_| None).collect();
        tiles.par_iter_mut().enumerate().for_each(|(idx, slot)| {
            let col = (idx as u32) % cols;
            let row = (idx as u32) / cols;
            let mut tile = Tile::empty(col * ts, row * ts, ts);
            for ty in 0..ts {
                let src_y = row * ts + ty;
                if src_y >= height {
                    break;
                }
                let row_start = (src_y as usize) * (width as usize) * 4;
                let max_x = ((col + 1) * ts).min(width);
                let copy_width = (max_x - col * ts) as usize;
                if copy_width == 0 {
                    break;
                }
                let src_offset = row_start + (col as usize) * (ts as usize) * 4;
                let dst_offset = (ty as usize) * (ts as usize) * 4;
                tile.pixels[dst_offset..dst_offset + copy_width * 4]
                    .copy_from_slice(&rgba[src_offset..src_offset + copy_width * 4]);
            }
            tile.dirty = false;
            *slot = Some(tile);
        });
        grid.tiles = tiles;
        Ok(grid)
    }

    /// Flatten back into a single RGBA8 buffer.
    #[must_use]
    pub fn to_image(&self) -> Vec<u8> {
        let stride = (self.width as usize) * 4;
        let mut out = vec![0u8; stride * (self.height as usize)];
        for row in 0..self.rows {
            for col in 0..self.cols {
                let idx = (row as usize) * (self.cols as usize) + (col as usize);
                let Some(tile) = &self.tiles[idx] else {
                    continue;
                };
                let ts = self.tile_size as usize;
                for ty in 0..ts {
                    let dst_y = (row as usize) * ts + ty;
                    if dst_y >= self.height as usize {
                        break;
                    }
                    let max_x = ((col as usize + 1) * ts).min(self.width as usize);
                    let copy_width = max_x - (col as usize) * ts;
                    if copy_width == 0 {
                        break;
                    }
                    let dst_offset = dst_y * stride + (col as usize) * ts * 4;
                    let src_offset = ty * ts * 4;
                    out[dst_offset..dst_offset + copy_width * 4]
                        .copy_from_slice(&tile.pixels[src_offset..src_offset + copy_width * 4]);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_validates_inputs() {
        assert!(TileGrid::new(0, 1, 8).is_err());
        assert!(TileGrid::new(1, 1, 0).is_err());
        let g = TileGrid::new(10, 10, 4).expect("valid");
        assert_eq!(g.cols(), 3);
        assert_eq!(g.rows(), 3);
    }

    #[test]
    fn from_image_to_image_roundtrip() {
        let w = 5u32;
        let h = 3u32;
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                pixels.push(x as u8);
                pixels.push(y as u8);
                pixels.push(0x80);
                pixels.push(0xFF);
            }
        }
        let g = TileGrid::from_image(&pixels, w, h, 2).expect("grid");
        let back = g.to_image();
        assert_eq!(back, pixels);
    }

    #[test]
    fn dirty_tracking() {
        let mut g = TileGrid::new(10, 10, 4).expect("grid");
        g.mark_dirty(0, 0).expect("mark");
        g.mark_dirty(2, 2).expect("mark");
        let mut d = g.dirty_tiles();
        d.sort_unstable();
        assert_eq!(d, vec![(0, 0), (2, 2)]);
        g.clear_dirty();
        assert!(g.dirty_tiles().is_empty());
    }
}
