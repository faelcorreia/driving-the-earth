//! Distant lights (L2): `cache/<id>/far_lights.bin`, one point per occupied cell of building centroids.
//! Written by `tilebake::far_lights`, read by the game. All values little-endian.
//!
//! Layout (version 1):
//! - header, 12 bytes: magic `FLT1` (4 bytes), version u32 (= 1), count u32 (number of records);
//! - `count` records of 14 bytes each: x f32 (m, map coordinates, as in the tiles), y f32, z f32
//!   (terrain height at the point, m), weight u8 (buildings in the cell, saturated at 255), hue u8
//!   (0..=255, the shader maps it to the sodium-to-warm-white range).
use std::io;
use std::path::Path;

pub const MAGIC: [u8; 4] = *b"FLT1";
pub const VERSION: u32 = 1;
pub const HEADER_BYTES: usize = 12;
pub const RECORD_BYTES: usize = 14;

/// One distant light.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FarLight {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub weight: u8,
    pub hue: u8,
}

/// Serializes the points in the layout above.
pub fn encode(points: &[FarLight]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_BYTES + points.len() * RECORD_BYTES);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(points.len() as u32).to_le_bytes());
    for p in points {
        out.extend_from_slice(&p.x.to_le_bytes());
        out.extend_from_slice(&p.y.to_le_bytes());
        out.extend_from_slice(&p.z.to_le_bytes());
        out.push(p.weight);
        out.push(p.hue);
    }
    out
}

/// Parses the layout above. Rejects a wrong magic, an unknown version and a size that does not match
/// the declared count.
pub fn decode(bytes: &[u8]) -> io::Result<Vec<FarLight>> {
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, format!("far_lights.bin: {m}"));
    if bytes.len() < HEADER_BYTES {
        return Err(bad("shorter than the header"));
    }
    if bytes[0..4] != MAGIC {
        return Err(bad("bad magic"));
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    if version != VERSION {
        return Err(bad("unknown version"));
    }
    let count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    if bytes.len() != HEADER_BYTES + count * RECORD_BYTES {
        return Err(bad("size does not match the count"));
    }
    let f32_at = |b: &[u8], o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    Ok(bytes[HEADER_BYTES..]
        .chunks_exact(RECORD_BYTES)
        .map(|r| FarLight {
            x: f32_at(r, 0),
            y: f32_at(r, 4),
            z: f32_at(r, 8),
            weight: r[12],
            hue: r[13],
        })
        .collect())
}

pub fn write_far_lights(path: &Path, points: &[FarLight]) -> io::Result<()> {
    std::fs::write(path, encode(points))
}

pub fn read_far_lights(path: &Path) -> io::Result<Vec<FarLight>> {
    decode(&std::fs::read(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<FarLight> {
        vec![
            FarLight { x: 1200.5, y: -3400.25, z: 912.0, weight: 3, hue: 0 },
            FarLight { x: -5000.0, y: 4200.0, z: 1100.75, weight: 255, hue: 255 },
        ]
    }

    #[test]
    fn round_trip_keeps_every_field() {
        let pts = sample();
        let bytes = encode(&pts);
        assert_eq!(bytes.len(), HEADER_BYTES + pts.len() * RECORD_BYTES);
        assert_eq!(decode(&bytes).unwrap(), pts);
    }

    #[test]
    fn empty_file_is_a_valid_header_only() {
        let bytes = encode(&[]);
        assert_eq!(bytes.len(), HEADER_BYTES);
        assert!(decode(&bytes).unwrap().is_empty());
    }

    #[test]
    fn bad_magic_version_or_size_is_rejected() {
        let mut bytes = encode(&sample());
        bytes[0] = b'X';
        assert!(decode(&bytes).is_err());
        let mut bytes = encode(&sample());
        bytes[4] = 2;
        assert!(decode(&bytes).is_err());
        let mut bytes = encode(&sample());
        bytes.pop();
        assert!(decode(&bytes).is_err());
        assert!(decode(&[0u8; 3]).is_err());
    }

    #[test]
    fn record_is_fourteen_bytes() {
        assert_eq!(encode(&sample()).len() - HEADER_BYTES, 2 * 14);
    }

    #[test]
    fn file_round_trip() {
        let dir = std::env::temp_dir().join(format!("far_lights_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("far_lights.bin");
        write_far_lights(&path, &sample()).unwrap();
        assert_eq!(read_far_lights(&path).unwrap(), sample());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
