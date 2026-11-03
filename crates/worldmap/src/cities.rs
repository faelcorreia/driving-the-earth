//! Which cities show at a zoom, and where their names go. A city shows once the zoom reaches its
//! tier (the largest from the start, the smallest only when close); cities are placed largest
//! first and a name is kept only if its box does not overlap a name already placed.
use crate::asset::City;
use crate::view::View;

/// Zoom steps (doublings above the whole world) at which each tier starts to show.
pub const TIER_MIN_STEPS: [f64; 4] = [0.0, 1.5, 3.0, 4.5];
/// Most names on screen at once.
pub const MAX_LABELS: usize = 60;
/// Gap between a city dot and its name, in pixels.
const LABEL_GAP_PX: f64 = 5.0;

/// A city on screen: its dot, and its name box `[x0, y0, x1, y1]` if the name was kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub city: usize,
    pub x: f64,
    pub y: f64,
    pub label: Option<[f64; 4]>,
}

pub fn tier_visible(tier: u8, steps: f64) -> bool {
    steps >= TIER_MIN_STEPS[tier.min(3) as usize]
}

/// Width of a name in pixels, estimated from its length (no font metrics, so it is pure).
pub fn label_width_px(name: &str, font_px: f64) -> f64 {
    name.chars().count() as f64 * font_px * 0.55
}

fn overlaps(a: &[f64; 4], b: &[f64; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// Cities on screen at the view's zoom, in the order of `cities` (largest first), with names
/// placed greedily. Cities outside the screen (with a margin for the name) are skipped.
pub fn place(cities: &[City], view: &View, font_px: f64) -> Vec<Placed> {
    let steps = view.zoom_steps();
    let copies = view.copies();
    let mut out = Vec::new();
    let mut boxes: Vec<[f64; 4]> = Vec::new();
    for (i, c) in cities.iter().enumerate() {
        if !tier_visible(c.tier, steps) {
            continue;
        }
        for &k in &copies {
            let (x, y) = view.to_screen(c.x + k as f64, c.y);
            if x < -font_px || x > view.width + font_px || y < -font_px || y > view.height + font_px {
                continue;
            }
            let w = label_width_px(&c.name, font_px);
            let b = [x + LABEL_GAP_PX, y - font_px / 2.0, x + LABEL_GAP_PX + w, y + font_px / 2.0];
            let fits = boxes.len() < MAX_LABELS
                && b[2] <= view.width
                && !boxes.iter().any(|o| overlaps(o, &b));
            if fits {
                boxes.push(b);
            }
            out.push(Placed { city: i, x, y, label: fits.then_some(b) });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::decode;

    fn city(name: &str, tier: u8, lat: f64, lon: f64) -> City {
        let (x, y) = crate::projection::to_unit(lat, lon);
        City { name: name.into(), tier, lat, lon, x, y }
    }

    #[test]
    fn only_the_largest_show_at_the_whole_world() {
        let cities = vec![city("A", 0, 0.0, 0.0), city("B", 1, 10.0, 10.0), city("C", 3, 20.0, 20.0)];
        let view = View::world(800.0, 600.0);
        let placed = place(&cities, &view, 12.0);
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].city, 0);
    }

    #[test]
    fn smaller_cities_show_as_you_zoom_in() {
        let cities = vec![city("A", 0, 0.0, 0.0), city("B", 1, 10.0, 10.0), city("C", 3, 5.0, 5.0)];
        let mut view = View::world(800.0, 600.0);
        view.center_on(0.0, 0.0, 2.0);
        let names: Vec<usize> = place(&cities, &view, 12.0).iter().map(|p| p.city).collect();
        assert!(names.contains(&0) && names.contains(&1));
        assert!(!names.contains(&2));
        view.center_on(0.0, 0.0, 5.0);
        let names: Vec<usize> = place(&cities, &view, 12.0).iter().map(|p| p.city).collect();
        assert!(names.contains(&2));
    }

    #[test]
    fn names_never_overlap_and_the_largest_keeps_its_name() {
        let cities = vec![
            city("Big", 0, 0.0, 0.0),
            city("Small", 0, 0.0, 0.01),
            city("Other", 0, 30.0, 30.0),
        ];
        let mut view = View::world(800.0, 600.0);
        view.center_on(0.0, 0.0, 3.0);
        let placed = place(&cities, &view, 12.0);
        let big = placed.iter().find(|p| p.city == 0).unwrap();
        let small = placed.iter().find(|p| p.city == 1).unwrap();
        assert!(big.label.is_some());
        assert!(small.label.is_none(), "same spot, second name dropped");
        let boxes: Vec<[f64; 4]> = placed.iter().filter_map(|p| p.label).collect();
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                assert!(!overlaps(a, b));
            }
        }
    }

    #[test]
    fn labels_are_capped() {
        let cities: Vec<City> = (0..200)
            .map(|i| city(&format!("C{i}"), 0, (i as f64 % 20.0) * 0.5, (i as f64 / 20.0) * 3.0))
            .collect();
        let mut view = View::world(1600.0, 1200.0);
        view.zoom_at(800.0, 600.0, 4.0);
        let placed = place(&cities, &view, 12.0);
        assert!(placed.iter().filter(|p| p.label.is_some()).count() <= MAX_LABELS);
    }

    #[test]
    fn real_asset_places_cities_at_world_zoom() {
        let data = decode(include_bytes!("../../../godot/assets/worldmap/world.bin")).unwrap();
        let view = View::world(1280.0, 720.0);
        let placed = place(&data.cities, &view, 12.0);
        assert!(!placed.is_empty());
        // only the largest tier at the whole-world zoom
        assert!(placed.iter().all(|p| data.cities[p.city].tier == 0));
        assert!(placed.iter().filter(|p| p.label.is_some()).count() <= MAX_LABELS);
    }
}
