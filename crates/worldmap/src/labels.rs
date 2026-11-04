//! Cache rules for the home picker's retained drawing (F17b, pure). The zoom step is the integer
//! number of doublings of the view scale: road geometry in screen pixels is valid for one step.
//! A label layout is valid for one zoom step and one set of displayed tiles; while the view only
//! pans, the layout is translated, and it is recomputed once the view has moved more than a few
//! pixels from where it was laid out.
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::tiles::TileId;

/// Zoom step of a scale in pixels per unit square: floor of its base-2 logarithm.
pub fn zoom_step(px_per_unit: f64) -> i32 {
    px_per_unit.max(f64::MIN_POSITIVE).log2().floor() as i32
}

/// Key of a label layout: the zoom step and the displayed tiles, in any order.
pub fn layout_key(step: i32, tiles: &[TileId]) -> u64 {
    let mut ids: Vec<(u8, u32, u32)> = tiles.iter().map(|t| (t.z, t.x, t.y)).collect();
    ids.sort_unstable();
    let mut h = DefaultHasher::new();
    step.hash(&mut h);
    ids.hash(&mut h);
    h.finish()
}

/// Whether a label layout made under `(key, anchor)` must be recomputed for the current `key` and
/// view `at` (screen pixels). Missing layout, another key, or a move beyond `max_move_px` means yes.
pub fn needs_relayout(
    prev: Option<(u64, [f64; 2])>,
    key: u64,
    at: [f64; 2],
    max_move_px: f64,
) -> bool {
    match prev {
        None => true,
        Some((k, anchor)) => {
            let (dx, dy) = (at[0] - anchor[0], at[1] - anchor[1]);
            k != key || (dx * dx + dy * dy).sqrt() > max_move_px
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_step_is_doublings() {
        assert_eq!(zoom_step(1.0), 0);
        assert_eq!(zoom_step(1.9), 0);
        assert_eq!(zoom_step(2.0), 1);
        assert_eq!(zoom_step(0.5), -1);
        assert!(zoom_step(0.0) < -1000);
    }

    #[test]
    fn layout_key_ignores_order_but_not_step_or_set() {
        let a = TileId { z: 10, x: 1, y: 2 };
        let b = TileId { z: 10, x: 3, y: 4 };
        let c = TileId { z: 10, x: 5, y: 6 };
        assert_eq!(layout_key(4, &[a, b]), layout_key(4, &[b, a]));
        assert_ne!(layout_key(4, &[a, b]), layout_key(5, &[a, b]));
        assert_ne!(layout_key(4, &[a, b]), layout_key(4, &[a, c]));
    }

    #[test]
    fn relayout_only_on_key_change_or_a_real_move() {
        let k = layout_key(4, &[TileId { z: 10, x: 1, y: 2 }]);
        assert!(needs_relayout(None, k, [0.0, 0.0], 8.0));
        let prev = Some((k, [100.0, 100.0]));
        assert!(!needs_relayout(prev, k, [103.0, 104.0], 8.0));
        assert!(needs_relayout(prev, k, [108.5, 100.0], 8.0));
        assert!(needs_relayout(prev, k ^ 1, [100.0, 100.0], 8.0));
    }
}
