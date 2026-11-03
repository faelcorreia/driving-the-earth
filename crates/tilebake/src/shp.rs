//! Streaming reader of polyline shapefiles (.shp + .dbf) for contour lines. The input is
//! untrusted and the 1 m file is ~700 MB, so every offset is checked and records are read one by one.
use std::fs;
use std::io::{self, BufReader, ErrorKind, Read};
use std::path::Path;

/// Vertices closer than this to the last kept vertex of the same part are skipped (the last one is kept).
pub const MIN_SPACING_M: f64 = 4.0;

fn bad(msg: &str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, msg.to_string())
}

fn rd<const N: usize>(b: &[u8], at: usize) -> io::Result<[u8; N]> {
    b.get(at..at + N)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| bad("shp: truncated record"))
}

fn le_i32(b: &[u8], at: usize) -> io::Result<i32> {
    Ok(i32::from_le_bytes(rd::<4>(b, at)?))
}

fn be_i32(b: &[u8], at: usize) -> io::Result<i32> {
    Ok(i32::from_be_bytes(rd::<4>(b, at)?))
}

fn le_f64(b: &[u8], at: usize) -> io::Result<f64> {
    Ok(f64::from_le_bytes(rd::<8>(b, at)?))
}

/// Values of the dbf field `name`, one per record. Non-numeric text gives NaN.
pub fn dbf_numbers(path: &Path, name: &str) -> io::Result<Vec<f64>> {
    let b = fs::read(path)?;
    if b.len() < 32 {
        return Err(bad("dbf: truncated header"));
    }
    let nrec = u32::from_le_bytes(rd::<4>(&b, 4)?) as usize;
    let hlen = u16::from_le_bytes(rd::<2>(&b, 8)?) as usize;
    let rlen = u16::from_le_bytes(rd::<2>(&b, 10)?) as usize;
    let mut field = None;
    let (mut off, mut pos) = (0usize, 32usize);
    while b.get(pos).is_some_and(|&c| c != 0x0D) {
        let d = b
            .get(pos..pos + 32)
            .ok_or_else(|| bad("dbf: truncated field table"))?;
        let fname = String::from_utf8_lossy(&d[..11]).trim_end_matches('\0').to_string();
        let flen = d[16] as usize;
        if fname == name {
            field = Some((off, flen));
        }
        off += flen;
        pos += 32;
    }
    let (foff, flen) = field.ok_or_else(|| bad("dbf: field not found"))?;
    let mut out = Vec::with_capacity(nrec);
    for r in 0..nrec {
        let start = hlen + r * rlen + 1 + foff;
        let s = b
            .get(start..start + flen)
            .ok_or_else(|| bad("dbf: truncated record"))?;
        let text = String::from_utf8_lossy(s);
        out.push(text.trim().replace(',', ".").parse::<f64>().unwrap_or(f64::NAN));
    }
    Ok(out)
}

/// Calls `visit(ax, ay, bx, by, z)` for each segment between consecutive kept vertices of every polyline of `shp` (x, y in the file's CRS;
/// z is the value of `z_field` in the dbf of the same record). Returns the number of parts read.
pub fn read_contours(
    shp: &Path,
    dbf: &Path,
    z_field: &str,
    mut visit: impl FnMut(f64, f64, f64, f64, f64),
) -> io::Result<usize> {
    let z = dbf_numbers(dbf, z_field)?;
    let mut f = BufReader::with_capacity(1 << 20, fs::File::open(shp)?);
    let mut head = [0u8; 100];
    f.read_exact(&mut head)?;
    if be_i32(&head, 0)? != 9994 {
        return Err(bad("shp: bad file code"));
    }
    let mut rec_hdr = [0u8; 8];
    let mut buf: Vec<u8> = Vec::new();
    let (mut rec, mut parts_read) = (0usize, 0usize);
    loop {
        match f.read_exact(&mut rec_hdr) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
        let len = usize::try_from(be_i32(&rec_hdr, 4)?).map_err(|_| bad("shp: bad record length"))? * 2;
        buf.resize(len, 0);
        f.read_exact(&mut buf)?;
        let zi = *z.get(rec).ok_or_else(|| bad("shp: more records than dbf rows"))?;
        rec += 1;
        let stype = le_i32(&buf, 0)?;
        if stype == 0 {
            continue;
        }
        if !matches!(stype, 3 | 13 | 23) {
            return Err(bad("shp: not a polyline file"));
        }
        let nparts = usize::try_from(le_i32(&buf, 36)?).map_err(|_| bad("shp: bad part count"))?;
        let npts = usize::try_from(le_i32(&buf, 40)?).map_err(|_| bad("shp: bad point count"))?;
        let pts_at = 44 + 4 * nparts;
        if pts_at + 16 * npts > buf.len() {
            return Err(bad("shp: truncated record"));
        }
        for p in 0..nparts {
            let start = usize::try_from(le_i32(&buf, 44 + 4 * p)?).map_err(|_| bad("shp: bad part index"))?;
            let end = if p + 1 < nparts {
                usize::try_from(le_i32(&buf, 44 + 4 * (p + 1))?).map_err(|_| bad("shp: bad part index"))?
            } else {
                npts
            };
            if start > end || end > npts {
                return Err(bad("shp: bad part index"));
            }
            let mut last: Option<(f64, f64)> = None;
            for k in start..end {
                let x = le_f64(&buf, pts_at + 16 * k)?;
                let y = le_f64(&buf, pts_at + 16 * k + 8)?;
                if !x.is_finite() || !y.is_finite() {
                    return Err(bad("shp: non-finite coordinate"));
                }
                let keep = match last {
                    None => true,
                    Some((lx, ly)) => k + 1 == end || (x - lx).hypot(y - ly) >= MIN_SPACING_M,
                };
                if keep {
                    if let Some((lx, ly)) = last {
                        visit(lx, ly, x, y, zi);
                    }
                    last = Some((x, y));
                }
            }
            parts_read += 1;
        }
    }
    Ok(parts_read)
}
