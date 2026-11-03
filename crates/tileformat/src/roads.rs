//! Road-only file of a baked map (`cache/<id>/roads.bin`). The 2D map reads this one file instead
//! of every tile, so it skips the heights and buildings. Little-endian layout:
//! - magic `DTEROAD1` (8 bytes) and the tile count (u32);
//! - per tile: x and y (i32), the piece count (u32), then per piece:
//!   highway and name (u16 length, then UTF-8), the point count (u32), and the points as f64 pairs.
//!
//! The terrain profile (`z`, `zl`, `zr`) is not stored: the map does not use it.
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::Polyline;

/// File name inside `cache/<id>/`.
pub const ROADS_FILE: &str = "roads.bin";
const MAGIC: &[u8; 8] = b"DTEROAD1";

/// Path of `cache/<id>/roads.bin` from the cache root (`cache/`).
pub fn roads_path(cache_root: &Path, id: &str) -> PathBuf {
    cache_root.join(id).join(ROADS_FILE)
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn put_str(out: &mut Vec<u8>, s: &str) -> io::Result<()> {
    let len = u16::try_from(s.len())
        .map_err(|_| invalid(format!("string too long for roads.bin: {} bytes", s.len())))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

/// Writes the road pieces of each tile `(x, y, roads)` to `path`. Returns the bytes written.
pub fn write_roads_file(path: &Path, tiles: &[(i32, i32, Vec<Polyline>)]) -> io::Result<u64> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    let count = u32::try_from(tiles.len()).map_err(|_| invalid("too many tiles for roads.bin"))?;
    out.extend_from_slice(&count.to_le_bytes());
    for (x, y, roads) in tiles {
        out.extend_from_slice(&x.to_le_bytes());
        out.extend_from_slice(&y.to_le_bytes());
        let n =
            u32::try_from(roads.len()).map_err(|_| invalid("too many road pieces in a tile"))?;
        out.extend_from_slice(&n.to_le_bytes());
        for road in roads {
            put_str(&mut out, &road.highway)?;
            put_str(&mut out, &road.name)?;
            let points = u32::try_from(road.points.len())
                .map_err(|_| invalid("too many points in a road"))?;
            out.extend_from_slice(&points.to_le_bytes());
            for p in &road.points {
                out.extend_from_slice(&p[0].to_le_bytes());
                out.extend_from_slice(&p[1].to_le_bytes());
            }
        }
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = io::BufWriter::new(fs::File::create(path)?);
    file.write_all(&out)?;
    file.flush()?;
    Ok(out.len() as u64)
}

/// Cursor over the bytes of a roads file; every read checks the length.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.buf.len())
            .ok_or_else(|| invalid("roads.bin is truncated"))?;
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> io::Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f64(&mut self) -> io::Result<f64> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn string(&mut self) -> io::Result<String> {
        let n = self.u16()? as usize;
        String::from_utf8(self.take(n)?.to_vec())
            .map_err(|_| invalid("roads.bin has a string that is not UTF-8"))
    }
}

/// Reads a roads file: for each tile `(x, y, roads)`, its road pieces with `z`, `zl` and `zr` empty.
pub fn read_roads_file(path: &Path) -> io::Result<Vec<(i32, i32, Vec<Polyline>)>> {
    let buf = fs::read(path)?;
    let mut r = Reader { buf: &buf, pos: 0 };
    if r.take(MAGIC.len())? != MAGIC {
        return Err(invalid("not a roads.bin file (bad magic)"));
    }
    let tiles = r.u32()? as usize;
    let mut out = Vec::with_capacity(tiles.min(1 << 16));
    for _ in 0..tiles {
        let (x, y) = (r.i32()?, r.i32()?);
        let pieces = r.u32()? as usize;
        let mut roads = Vec::with_capacity(pieces.min(1 << 16));
        for _ in 0..pieces {
            let highway = r.string()?;
            let name = r.string()?;
            let n = r.u32()? as usize;
            let mut points = Vec::with_capacity(n.min(1 << 20));
            for _ in 0..n {
                points.push([r.f64()?, r.f64()?]);
            }
            roads.push(Polyline {
                highway,
                name,
                points,
                z: Vec::new(),
                zl: Vec::new(),
                zr: Vec::new(),
            });
        }
        out.push((x, y, roads));
    }
    if r.pos != buf.len() {
        return Err(invalid("roads.bin has bytes after the last tile"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn road(highway: &str, name: &str, points: &[[f64; 2]]) -> Polyline {
        Polyline {
            highway: highway.into(),
            name: name.into(),
            points: points.to_vec(),
            z: vec![],
            zl: vec![],
            zr: vec![],
        }
    }

    fn temp_file(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("roads-{tag}-{}-{nanos}", std::process::id()))
            .join(ROADS_FILE)
    }

    fn sample() -> Vec<(i32, i32, Vec<Polyline>)> {
        vec![
            (
                -3,
                12,
                vec![
                    road(
                        "primary",
                        "Avenida Afonso Pena",
                        &[[0.5, -1.25], [250.0, 3.0]],
                    ),
                    road("service", "", &[[1.0, 1.0], [2.0, 2.0], [3.0, 1.0]]),
                ],
            ),
            (0, 0, vec![]),
            (
                4,
                -1,
                vec![road(
                    "residential",
                    "Rua Exemplo",
                    &[[10.0, 10.0], [20.0, 10.0]],
                )],
            ),
        ]
    }

    #[test]
    fn round_trip_keeps_class_name_and_points() {
        let path = temp_file("rt");
        let tiles = sample();
        let bytes = write_roads_file(&path, &tiles).unwrap();
        assert_eq!(bytes, fs::metadata(&path).unwrap().len());
        let back = read_roads_file(&path).unwrap();
        assert_eq!(back.len(), tiles.len());
        for ((x, y, roads), (bx, by, broads)) in tiles.iter().zip(&back) {
            assert_eq!((x, y), (bx, by));
            assert_eq!(roads.len(), broads.len());
            for (r, b) in roads.iter().zip(broads) {
                assert_eq!(
                    (&r.highway, &r.name, &r.points),
                    (&b.highway, &b.name, &b.points)
                );
                assert!(b.z.is_empty() && b.zl.is_empty() && b.zr.is_empty());
            }
        }
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn empty_map_round_trips() {
        let path = temp_file("empty");
        write_roads_file(&path, &[]).unwrap();
        assert!(read_roads_file(&path).unwrap().is_empty());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn bad_magic_and_truncation_are_errors() {
        let path = temp_file("bad");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"NOTROADS\0\0\0\0").unwrap();
        assert_eq!(
            read_roads_file(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        write_roads_file(&path, &sample()).unwrap();
        let full = fs::read(&path).unwrap();
        fs::write(&path, &full[..full.len() - 5]).unwrap();
        let err = read_roads_file(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("truncated"), "{err}");
        fs::write(&path, [full.as_slice(), &[0u8]].concat()).unwrap();
        assert!(
            read_roads_file(&path).is_err(),
            "trailing bytes are rejected"
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn path_is_inside_the_map_folder() {
        assert_eq!(
            roads_path(Path::new("cache"), "casa-10km"),
            Path::new("cache/casa-10km/roads.bin")
        );
    }
}
