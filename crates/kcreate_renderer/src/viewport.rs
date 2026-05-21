//! Viewport: pan + zoom transform from scene space to pixel space.
//!
//! `viewport.pan` is the scene-space point that maps to (0, 0) in pixel space.
//! `viewport.zoom` is the pixels-per-scene-unit scale (1.0 = 1:1).

use serde::{Deserialize, Serialize};

use crate::geometry::{Point2, Rect, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    pub pan: Vec2,
    pub zoom: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

impl Viewport {
    pub const fn new(pan: Vec2, zoom: f32) -> Self {
        Self {
            pan,
            zoom: zoom.max(f32::EPSILON),
        }
    }

    pub const fn set_pan(&mut self, pan: Vec2) {
        self.pan = pan;
    }

    pub const fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom.max(f32::EPSILON);
    }

    /// Map a scene-space point to pixel space.
    pub fn scene_to_pixel(&self, p: Point2) -> Point2 {
        Point2::new(
            (p.x - self.pan.x) * self.zoom,
            (p.y - self.pan.y) * self.zoom,
        )
    }

    /// Map a pixel-space point back to scene space.
    pub fn pixel_to_scene(&self, p: Point2) -> Point2 {
        Point2::new(p.x / self.zoom + self.pan.x, p.y / self.zoom + self.pan.y)
    }

    /// Transform a scene-space rect into pixel space.
    pub fn scene_to_pixel_rect(&self, r: Rect) -> Rect {
        let p = self.scene_to_pixel(Point2::new(r.x, r.y));
        Rect::new(p.x, p.y, r.width * self.zoom, r.height * self.zoom)
    }

    /// What region of scene space is currently visible at the given pixel size.
    pub fn visible_scene_rect(&self, pixel_size: (u32, u32)) -> Rect {
        let (w, h) = pixel_size;
        Rect::new(
            self.pan.x,
            self.pan.y,
            w as f32 / self.zoom,
            h as f32 / self.zoom,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_to_pixel_round_trips() {
        let vp = Viewport::new(Vec2::new(10.0, 20.0), 2.0);
        let p = Point2::new(15.0, 25.0);
        let pixel = vp.scene_to_pixel(p);
        assert_eq!(pixel, Point2::new(10.0, 10.0));
        let back = vp.pixel_to_scene(pixel);
        assert!((back.x - p.x).abs() < 1e-5);
        assert!((back.y - p.y).abs() < 1e-5);
    }

    #[test]
    fn visible_scene_rect_accounts_for_zoom() {
        let vp = Viewport::new(Vec2::new(0.0, 0.0), 2.0);
        let r = vp.visible_scene_rect((100, 50));
        assert_eq!(r, Rect::new(0.0, 0.0, 50.0, 25.0));
    }

    #[test]
    fn zoom_is_clamped_to_positive() {
        let vp = Viewport::new(Vec2::ZERO, -1.0);
        assert!(vp.zoom > 0.0);
    }
}
