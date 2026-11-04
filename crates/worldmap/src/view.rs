//! Zoom and pan of the world map. The view is a point of the unit square (`cx`, `cy`, the center
//! on screen) and a scale in pixels per unit. The whole world fits at the widest zoom; zooming in
//! goes up to `MAX_ZOOM_STEPS` doublings further. Pure math, tested.
use crate::projection::{clamp_lat, from_unit, to_unit};

/// Zoom levels above "whole world on screen" (2^12 is about 1 m per pixel at 4000 px wide).
pub const MAX_ZOOM_STEPS: f64 = 15.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub cx: f64,
    pub cy: f64,
    pub px_per_unit: f64,
    pub width: f64,
    pub height: f64,
}

impl View {
    /// The whole world, centered, on a screen of this size in pixels.
    pub fn world(width: f64, height: f64) -> View {
        let mut v = View {
            cx: 0.5,
            cy: 0.5,
            px_per_unit: 1.0,
            width,
            height,
        };
        v.px_per_unit = v.min_scale();
        v
    }

    /// Scale at which the whole world fits (the shorter side of the screen).
    pub fn min_scale(&self) -> f64 {
        self.width.min(self.height).max(1.0)
    }

    /// Zoom in doublings above the whole world, 0 to `MAX_ZOOM_STEPS`.
    pub fn zoom_steps(&self) -> f64 {
        (self.px_per_unit / self.min_scale()).log2()
    }

    /// Unit square point to screen pixels.
    pub fn to_screen(&self, x: f64, y: f64) -> (f64, f64) {
        (
            (x - self.cx) * self.px_per_unit + self.width / 2.0,
            (y - self.cy) * self.px_per_unit + self.height / 2.0,
        )
    }

    /// Screen pixels to unit square point.
    pub fn to_unit(&self, sx: f64, sy: f64) -> (f64, f64) {
        (
            (sx - self.width / 2.0) / self.px_per_unit + self.cx,
            (sy - self.height / 2.0) / self.px_per_unit + self.cy,
        )
    }

    /// Latitude and longitude under a screen pixel.
    pub fn lat_lon_at(&self, sx: f64, sy: f64) -> (f64, f64) {
        let (x, y) = self.to_unit(sx, sy);
        from_unit(x, y)
    }

    /// Screen pixel of a latitude and longitude (first world copy).
    pub fn screen_of(&self, lat: f64, lon: f64) -> (f64, f64) {
        let (x, y) = to_unit(clamp_lat(lat), lon);
        self.to_screen(x, y)
    }

    /// Zoom by `factor` keeping the point under the screen pixel (`sx`, `sy`) fixed.
    pub fn zoom_at(&mut self, sx: f64, sy: f64, factor: f64) {
        let (ux, uy) = self.to_unit(sx, sy);
        let min = self.min_scale();
        self.px_per_unit = (self.px_per_unit * factor).clamp(min, min * 2f64.powf(MAX_ZOOM_STEPS));
        self.cx = ux - (sx - self.width / 2.0) / self.px_per_unit;
        self.cy = uy - (sy - self.height / 2.0) / self.px_per_unit;
        self.clamp_center();
    }

    /// Moves the map by a pixel offset (drag).
    pub fn pan(&mut self, dx: f64, dy: f64) {
        self.cx -= dx / self.px_per_unit;
        self.cy -= dy / self.px_per_unit;
        self.clamp_center();
    }

    /// Centers the view on a latitude and longitude at a zoom level (in doublings).
    pub fn center_on(&mut self, lat: f64, lon: f64, steps: f64) {
        let min = self.min_scale();
        self.px_per_unit = min * 2f64.powf(steps.clamp(0.0, MAX_ZOOM_STEPS));
        let (x, y) = to_unit(clamp_lat(lat), lon);
        self.cx = x;
        self.cy = y;
        self.clamp_center();
    }

    /// Keeps the vertical center inside the world; the horizontal wraps around.
    fn clamp_center(&mut self) {
        self.cx = self.cx.rem_euclid(1.0);
        let half = self.height / 2.0 / self.px_per_unit;
        self.cy = if 2.0 * half >= 1.0 {
            0.5
        } else {
            self.cy.clamp(half, 1.0 - half)
        };
    }

    /// Horizontal copies of the world (`k` such that the copy spans x in [k, k + 1]) that reach
    /// the screen. Usually one or two.
    pub fn copies(&self) -> Vec<i32> {
        (-2..=2)
            .filter(|&k| {
                let (x0, _) = self.to_screen(k as f64, 0.0);
                let (x1, _) = self.to_screen(k as f64 + 1.0, 0.0);
                x1 >= 0.0 && x0 <= self.width
            })
            .collect()
    }

    /// Whether a unit-square box `[x0, y0, x1, y1]`, shifted by `k` world copies, touches the screen.
    pub fn box_visible(&self, k: i32, b: [f64; 4]) -> bool {
        let (sx0, sy0) = self.to_screen(b[0] + k as f64, b[1]);
        let (sx1, sy1) = self.to_screen(b[2] + k as f64, b[3]);
        sx1 >= 0.0 && sx0 <= self.width && sy1 >= 0.0 && sy0 <= self.height
    }

    /// Screen points of a unit-square polyline shifted by `k` copies, with points closer than
    /// `min_px` to the last kept one dropped (the last point is kept too). This is the zoom-based
    /// simplification: at a wide zoom most of the coastline points fall inside one pixel.
    pub fn screen_points(&self, pts: &[[f64; 2]], k: i32, min_px: f64) -> Vec<[f64; 2]> {
        let mut out: Vec<[f64; 2]> = Vec::new();
        for (i, p) in pts.iter().enumerate() {
            let s = self.to_screen(p[0] + k as f64, p[1]);
            let s = [s.0, s.1];
            let Some(prev) = out.last().copied() else {
                out.push(s);
                continue;
            };
            if (s[0] - prev[0]).hypot(s[1] - prev[1]) >= min_px {
                out.push(s);
            } else if i + 1 == pts.len() && out.len() > 1 {
                // The last point is kept (a closed ring ends on its first point), so the point
                // before it gives way instead of leaving a spike a pixel long.
                *out.last_mut().unwrap() = s;
            } else if i + 1 == pts.len() {
                out.push(s);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_whole_world() {
        let v = View::world(800.0, 600.0);
        assert_eq!(v.px_per_unit, 600.0);
        assert!((v.zoom_steps()).abs() < 1e-12);
        // the neighbouring copies show in the side margins of the screen
        assert_eq!(v.copies(), vec![-1, 0, 1]);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let mut v = View::world(800.0, 600.0);
        let (sx, sy) = (250.0, 180.0);
        let before = v.lat_lon_at(sx, sy);
        v.zoom_at(sx, sy, 3.0);
        let after = v.lat_lon_at(sx, sy);
        assert!((before.0 - after.0).abs() < 1e-9 && (before.1 - after.1).abs() < 1e-9);
        assert!(v.zoom_steps() > 1.5);
    }

    #[test]
    fn zoom_is_limited() {
        let mut v = View::world(800.0, 600.0);
        for _ in 0..200 {
            v.zoom_at(400.0, 300.0, 1.5);
        }
        assert!((v.zoom_steps() - MAX_ZOOM_STEPS).abs() < 1e-9);
        for _ in 0..200 {
            v.zoom_at(400.0, 300.0, 0.5);
        }
        assert!((v.zoom_steps()).abs() < 1e-9);
    }

    #[test]
    fn vertical_pan_stops_at_the_limits() {
        let mut v = View::world(800.0, 600.0);
        v.zoom_at(400.0, 300.0, 4.0);
        v.pan(0.0, 1e9);
        let half = 300.0 / v.px_per_unit;
        assert!((v.cy - half).abs() < 1e-12);
        v.pan(0.0, -1e9);
        assert!((v.cy - (1.0 - half)).abs() < 1e-12);
    }

    #[test]
    fn horizontal_pan_wraps() {
        let mut v = View::world(800.0, 600.0);
        v.zoom_at(400.0, 300.0, 4.0);
        v.pan(-v.px_per_unit * 0.9, 0.0);
        assert!((v.cx - 0.4).abs() < 1e-9, "cx {}", v.cx);
    }

    #[test]
    fn center_on_puts_place_at_screen_center() {
        let mut v = View::world(800.0, 600.0);
        v.center_on(-19.9, -43.9, 4.0);
        let (sx, sy) = v.screen_of(-19.9, -43.9);
        assert!((sx - 400.0).abs() < 1e-6 && (sy - 300.0).abs() < 1e-6);
        assert!((v.zoom_steps() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn copies_and_box_visibility() {
        let mut v = View::world(800.0, 600.0);
        v.zoom_at(400.0, 300.0, 4.0);
        v.center_on(0.0, 179.0, 4.0);
        let c = v.copies();
        assert!(c.contains(&0) && c.contains(&1), "copies {c:?}");
        assert!(v.box_visible(0, [0.99, 0.49, 1.0, 0.51]));
        assert!(!v.box_visible(0, [0.0, 0.0, 0.1, 0.1]));
    }

    #[test]
    fn screen_points_drop_sub_pixel_points() {
        let v = View::world(800.0, 600.0);
        let pts = [[0.5, 0.5], [0.5 + 1e-6, 0.5], [0.6, 0.5], [0.6 + 1e-6, 0.5]];
        let out = v.screen_points(&pts, 0, 1.5);
        // the first point, then the 0.6 point; the last sub-pixel point replaces it
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out.last().unwrap()[0], v.to_screen(0.6 + 1e-6, 0.5).0);
    }
}
