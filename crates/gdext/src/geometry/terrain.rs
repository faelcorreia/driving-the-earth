//! Terrain height of a tile: the one reader the geometry and the map use. Pure Rust (no Godot
//! types). Imports `tileformat` data only.
//!
//! A tile with a recorded heightfield (`fine_height`, 2,5 m) is read from it, on the same triangles
//! as the 10 m grid (`height`). A tile without one (baked before it existed) or with a field that
//! does not fit its tile falls back to the grid, which is the height the game drew before the field.
use tileformat::terrain::sample_height as sample_grid;
use tileformat::{Heightfield, Tile};

/// Terrain height at map point `(x, y)` of `tile` (`tile_m` meters across).
pub fn sample_height(tile: &Tile, tile_m: f64, x: f64, y: f64) -> f32 {
    match &tile.fine_height {
        Some(fine) if fine_fits(fine) => fine_sample(
            fine,
            tile_m,
            x - tile.x as f64 * tile_m,
            y - tile.y as f64 * tile_m,
        ),
        _ => sample_grid(tile, tile_m, x, y),
    }
}

/// A field is usable when it is square, with at least two samples per side, and holds every sample.
fn fine_fits(f: &Heightfield) -> bool {
    f.cols >= 2 && f.rows == f.cols && f.cm.len() == (f.cols as usize) * (f.cols as usize)
}

/// `sample_height` of a field at `(lx, ly)` meters from the tile corner. The same triangle split as
/// `tileformat::terrain::sample_local`, which reads the grid.
fn fine_sample(f: &Heightfield, tile_m: f64, lx: f64, ly: f64) -> f32 {
    let n = f.cols as usize;
    let cell = tile_m / (n - 1) as f64;
    let u = lx / cell;
    let v = ly / cell;
    let j0 = (u.floor().max(0.0) as usize).min(n - 2);
    let i0 = (v.floor().max(0.0) as usize).min(n - 2);
    let (fu, fv) = (u - j0 as f64, v - i0 as f64);
    let at = |i: usize, j: usize| f.meters(i * n + j) as f64;
    let (a, b, c, d) = (
        at(i0, j0),
        at(i0, j0 + 1),
        at(i0 + 1, j0),
        at(i0 + 1, j0 + 1),
    );
    let h = if fu + fv <= 1.0 {
        // Triangle a, b, c.
        a + (b - a) * fu + (c - a) * fv
    } else {
        // Triangle b, c, d.
        d + (c - d) * (1.0 - fu) + (b - d) * (1.0 - fv)
    };
    h as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::tests::{grid, hill, tile};
    use tileformat::terrain::fine_heightfield;

    /// The tile with a heightfield made from its grid, and the same tile without one.
    fn with_and_without_field(t: &Tile, tile_m: f64) -> (Tile, Tile) {
        let fine = fine_heightfield(&t.height, tile_m).expect("fine heightfield");
        (
            Tile {
                signs: Vec::new(),
                fine_height: Some(fine),
                ..t.clone()
            },
            t.clone(),
        )
    }

    /// A lattice of points over a 250 m tile, off the 2,5 m and 10 m vertices.
    fn lattice(tile_m: f64) -> impl Iterator<Item = (f64, f64)> {
        (0..60).flat_map(move |i| {
            (0..60).map(move |j| {
                (
                    0.37 + i as f64 * tile_m / 60.0,
                    0.71 + j as f64 * tile_m / 60.0,
                )
            })
        })
    }

    #[test]
    fn a_tile_reads_its_field_where_it_has_one() {
        let t = tile(0, 0, grid(26, |_, _| 0.0), Vec::new());
        let fine = Heightfield::from_meters(101, 101, &vec![3.0; 101 * 101]).unwrap();
        let with = Tile {
            signs: Vec::new(),
            fine_height: Some(fine),
            ..t.clone()
        };
        assert!((sample_height(&with, 250.0, 100.0, 40.0) - 3.0).abs() < 0.01);
        assert_eq!(sample_height(&t, 250.0, 100.0, 40.0), 0.0);
    }

    #[test]
    fn a_field_is_read_at_its_own_samples() {
        let t = tile(
            0,
            0,
            grid(26, |u, v| hill(u * 250.0, v * 250.0)),
            Vec::new(),
        );
        let (with, _) = with_and_without_field(&t, 250.0);
        let fine = with.fine_height.as_ref().unwrap();
        for i in 0..101 {
            for j in 0..101 {
                let (x, y) = (j as f64 * 2.5, i as f64 * 2.5);
                let got = sample_height(&with, 250.0, x, y);
                assert!(
                    (got - fine.meters(i * 101 + j)).abs() < 1e-4,
                    "({i},{j}): {got} vs {}",
                    fine.meters(i * 101 + j)
                );
            }
        }
    }

    #[test]
    fn a_tile_with_and_without_field_agree_within_5_cm() {
        let t = tile(
            0,
            0,
            grid(26, |u, v| hill(u * 250.0, v * 250.0)),
            Vec::new(),
        );
        let (with, without) = with_and_without_field(&t, 250.0);
        let worst = lattice(250.0)
            .map(|(x, y)| {
                (sample_height(&with, 250.0, x, y) - sample_height(&without, 250.0, x, y)).abs()
            })
            .fold(0.0f32, f32::max);
        assert!(worst <= 0.05, "worst difference {worst} m");
    }

    #[test]
    fn a_malformed_field_falls_back_to_the_grid() {
        let t = tile(
            0,
            0,
            grid(26, |u, v| hill(u * 250.0, v * 250.0)),
            Vec::new(),
        );
        let want = sample_height(&t, 250.0, 61.3, 17.9);
        for bad in [
            Heightfield {
                cols: 1,
                rows: 1,
                base_cm: 0,
                cm: vec![0],
            },
            Heightfield {
                cols: 3,
                rows: 2,
                base_cm: 0,
                cm: vec![0; 6],
            },
            Heightfield {
                cols: 3,
                rows: 3,
                base_cm: 0,
                cm: vec![0; 8],
            },
        ] {
            let with = Tile {
                signs: Vec::new(),
                fine_height: Some(bad),
                ..t.clone()
            };
            assert_eq!(sample_height(&with, 250.0, 61.3, 17.9), want);
        }
    }

    #[test]
    fn the_field_sampler_is_the_grid_sampler() {
        let values: Vec<f32> = (0..101 * 101)
            .map(|k| 900.0 + ((k * 37) % 101) as f32 * 0.05 + ((k / 101) as f32).sin())
            .collect();
        let f = Heightfield::from_meters(101, 101, &values).unwrap();
        let as_grid = tileformat::HeightGrid {
            cols: 101,
            rows: 101,
            values: (0..101 * 101).map(|k| f.meters(k)).collect(),
        };
        for (x, y) in [
            (0.0, 0.0),
            (1.3, 2.9),
            (120.4, 33.1),
            (249.9, 249.9),
            (250.0, 10.0),
        ] {
            let a = fine_sample(&f, 250.0, x, y);
            let b = tileformat::terrain::sample_local(&as_grid, 250.0, x, y);
            assert!((a - b).abs() < 1e-5, "({x},{y}): {a} vs {b}");
        }
    }
}
