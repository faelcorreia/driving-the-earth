//! Decoding of `godot/assets/worldmap/world.bin` (format in `scripts/build_worldmap.py`). Every
//! count and string length is checked against the bytes left before anything is allocated. The
//! coordinates are stored as lon/lat and kept as unit-square points, with a box per piece.
use crate::projection::{to_unit, MAX_LAT};

#[derive(Debug, Clone, PartialEq)]
pub struct Ring {
    /// True for water inside land (a lake): drawn with the background color.
    pub hole: bool,
    pub pts: Vec<[f64; 2]>,
    /// `[x0, y0, x1, y1]` of the points in the unit square.
    pub bbox: [f64; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub pts: Vec<[f64; 2]>,
    pub bbox: [f64; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct City {
    pub name: String,
    /// 0 is the largest (over 5 million people), 3 the smallest.
    pub tier: u8,
    pub lat: f64,
    pub lon: f64,
    /// Unit-square position.
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorldData {
    pub rings: Vec<Ring>,
    pub lines: Vec<Line>,
    /// Largest first.
    pub cities: Vec<City>,
}

#[derive(Debug, PartialEq)]
pub enum AssetError {
    BadMagic,
    BadVersion(u16),
    Truncated,
    BadCoord,
    BadTier(u8),
    BadName,
    Trailing(usize),
}

/// Longitude limit of unwrapped pieces (a ring can run past 180 to stay whole).
const LON_LIMIT: f64 = 720.0;
const COORD_SCALE: f64 = 100_000.0;

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], AssetError> {
        let end = self.pos.checked_add(n).ok_or(AssetError::Truncated)?;
        let slice = self.bytes.get(self.pos..end).ok_or(AssetError::Truncated)?;
        self.pos = end;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8, AssetError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, AssetError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, AssetError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32, AssetError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    /// `n` lon/lat pairs as unit-square points, after checking that the bytes exist.
    fn points(&mut self, n: usize) -> Result<Vec<[f64; 2]>, AssetError> {
        if n.checked_mul(8).ok_or(AssetError::Truncated)? > self.bytes.len() - self.pos {
            return Err(AssetError::Truncated);
        }
        let mut pts = Vec::with_capacity(n);
        for _ in 0..n {
            let lon = self.i32()? as f64 / COORD_SCALE;
            let lat = self.i32()? as f64 / COORD_SCALE;
            if lat.abs() > MAX_LAT + 1e-4 || lon.abs() > LON_LIMIT {
                return Err(AssetError::BadCoord);
            }
            let (x, y) = to_unit(lat, lon);
            pts.push([x, y]);
        }
        Ok(pts)
    }
}

fn bbox_of(pts: &[[f64; 2]]) -> [f64; 4] {
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for p in pts {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    b
}

/// Decodes the whole asset. Fails on a wrong magic or version, on counts that do not fit in the
/// bytes, on coordinates out of range, on an unknown tier, on a name that is not UTF-8, and on
/// trailing bytes.
pub fn decode(bytes: &[u8]) -> Result<WorldData, AssetError> {
    let mut r = Reader { bytes, pos: 0 };
    if r.take(4)? != b"DTEW" {
        return Err(AssetError::BadMagic);
    }
    let version = r.u16()?;
    if version != 1 {
        return Err(AssetError::BadVersion(version));
    }
    r.u16()?; // reserved
    let mut data = WorldData::default();
    for _ in 0..r.u32()? {
        let hole = r.u8()? != 0;
        let n = r.u32()? as usize;
        let pts = r.points(n)?;
        let bbox = bbox_of(&pts);
        data.rings.push(Ring { hole, pts, bbox });
    }
    for _ in 0..r.u32()? {
        let n = r.u32()? as usize;
        let pts = r.points(n)?;
        let bbox = bbox_of(&pts);
        data.lines.push(Line { pts, bbox });
    }
    for _ in 0..r.u32()? {
        let tier = r.u8()?;
        if tier > 3 {
            return Err(AssetError::BadTier(tier));
        }
        let lon = r.i32()? as f64 / COORD_SCALE;
        let lat = r.i32()? as f64 / COORD_SCALE;
        let len = r.u8()? as usize;
        let name = std::str::from_utf8(r.take(len)?).map_err(|_| AssetError::BadName)?;
        if lat.abs() > MAX_LAT + 1e-4 || lon.abs() > 180.0 {
            return Err(AssetError::BadCoord);
        }
        let (x, y) = to_unit(lat, lon);
        data.cities.push(City {
            name: name.to_string(),
            tier,
            lat,
            lon,
            x,
            y,
        });
    }
    if r.pos != bytes.len() {
        return Err(AssetError::Trailing(bytes.len() - r.pos));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed asset, written by `scripts/build_worldmap.py`.
    const WORLD: &[u8] = include_bytes!("../../../godot/assets/worldmap/world.bin");

    fn header(rings: u32) -> Vec<u8> {
        let mut b = b"DTEW".to_vec();
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&rings.to_le_bytes());
        b
    }

    fn point(b: &mut Vec<u8>, lon: i32, lat: i32) {
        b.extend_from_slice(&lon.to_le_bytes());
        b.extend_from_slice(&lat.to_le_bytes());
    }

    #[test]
    fn committed_asset_decodes() {
        let w = decode(WORLD).expect("world.bin decodes");
        assert_eq!(w.rings.len(), 1016);
        assert_eq!(w.lines.len(), 393);
        assert_eq!(w.cities.len(), 7342);
        assert!(w.cities.windows(2).all(|p| p[0].tier <= p[1].tier));
        assert!(w
            .cities
            .iter()
            .any(|c| c.name == "São Paulo" || c.name == "Sao Paulo"));
        for r in &w.rings {
            assert!(r.bbox[0] <= r.bbox[2] && r.bbox[1] <= r.bbox[3]);
        }
    }

    #[test]
    fn small_asset_round_trip() {
        let mut b = header(1);
        b.push(0);
        b.extend_from_slice(&4u32.to_le_bytes());
        for (lon, lat) in [(0, 0), (1_000_000, 0), (1_000_000, 1_000_000), (0, 0)] {
            point(&mut b, lon, lat);
        }
        b.extend_from_slice(&0u32.to_le_bytes()); // lines
        b.extend_from_slice(&1u32.to_le_bytes()); // cities
        b.push(2);
        point(&mut b, 1_000_000, 1_000_000);
        b.push(4);
        b.extend_from_slice("Belo".as_bytes());
        let w = decode(&b).unwrap();
        assert_eq!(w.rings.len(), 1);
        assert_eq!(w.rings[0].pts.len(), 4);
        assert!(!w.rings[0].hole);
        assert_eq!(w.cities[0].name, "Belo");
        assert_eq!(w.cities[0].tier, 2);
        assert!((w.cities[0].lon - 10.0).abs() < 1e-9);
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(
            decode(b"XXXX\x01\x00\x00\x00\x00\x00\x00\x00"),
            Err(AssetError::BadMagic)
        );
        let mut v2 = header(0);
        v2[4] = 2;
        assert_eq!(decode(&v2), Err(AssetError::BadVersion(2)));
        // claims a million points in a few bytes
        let mut huge = header(1);
        huge.push(0);
        huge.extend_from_slice(&1_000_000u32.to_le_bytes());
        assert_eq!(decode(&huge), Err(AssetError::Truncated));
        let mut trailing = header(0);
        trailing.extend_from_slice(&[0; 8]); // lines = 0, cities = 0
        trailing.extend_from_slice(&[0; 8]);
        assert_eq!(decode(&trailing), Err(AssetError::Trailing(8)));
        assert_eq!(decode(&WORLD[..100]), Err(AssetError::Truncated));
    }

    #[test]
    fn rejects_latitude_past_the_limit() {
        let mut b = header(1);
        b.push(0);
        b.extend_from_slice(&2u32.to_le_bytes());
        point(&mut b, 0, 0);
        point(&mut b, 0, 90 * 100_000);
        assert_eq!(decode(&b), Err(AssetError::BadCoord));
    }
}
