//! DEM Copernicus GLO-30 (DSM) -> alturas da grade de cada tile.
//!
//! Grade do tile: plano local em metros, origem no centro do `meta.json`
//! (x = leste, y = norte). O tile `(x, y)` cobre `[x*tile, (x+1)*tile]` x
//! `[y*tile, (y+1)*tile]`. A grade tem amostras nos cantos, com passo `CELULA_M`:
//! `cols = rows = tile/CELULA_M + 1`, `values[row * cols + col]` com `row` ao norte
//! e `col` ao leste. Tiles vizinhos compartilham a borda.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::fmt::Display;

use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;
use tileformat::{HeightGrid, MapMeta};

const USER_AGENT: &str = "driving-the-earth/0.1 (tilebake; projeto pessoal)";
const COPERNICUS_BASE: &str = "https://copernicus-dem-30m.s3.amazonaws.com";
/// Passo da grade do tile, em metros.
pub const CELULA_M: f64 = 10.0;
/// Raio médio da Terra, para o plano local.
const RAIO_TERRA_M: f64 = 6_371_000.0;
/// Valores abaixo disso (e não finitos) são "sem dado".
const NODATA_LIMITE: f32 = -9000.0;
/// Tags GeoTIFF: ModelPixelScale e ModelTiepoint.
const TAG_PIXEL_SCALE: u16 = 33550;
const TAG_TIEPOINT: u16 = 33922;

/// Relevo lido do GeoTIFF, já suavizado. Pixel (c, r) tem centro em
/// `lon0 + (c + 0.5) * passo_lon` e `lat0 - (r + 0.5) * passo_lat`.
#[derive(Debug, Clone)]
pub struct Raster {
    pub largura: usize,
    pub altura: usize,
    /// Canto superior esquerdo do raster, em graus.
    pub lon0: f64,
    pub lat0: f64,
    /// Tamanho do pixel em graus (positivos).
    pub passo_lon: f64,
    pub passo_lat: f64,
    /// Alturas linha-major; `NaN` onde não há dado.
    pub valores: Vec<f32>,
}

/// Grade do mapa: converte coordenadas do tile em lat/lon e define o tile.
#[derive(Debug, Clone, Copy)]
pub struct Grade {
    pub lat0: f64,
    pub lon0: f64,
    pub tile_m: f64,
    pub celula_m: f64,
}

impl Grade {
    pub fn de_meta(meta: &MapMeta) -> Self {
        Grade {
            lat0: meta.center_lat,
            lon0: meta.center_lon,
            tile_m: meta.tile_size_m,
            celula_m: CELULA_M,
        }
    }

    /// (lat, lon) do ponto `(x_m, y_m)` do plano local (leste, norte).
    pub fn latlon(&self, x_m: f64, y_m: f64) -> (f64, f64) {
        let m_lat = RAIO_TERRA_M.to_radians();
        let m_lon = m_lat * self.lat0.to_radians().cos();
        (self.lat0 + y_m / m_lat, self.lon0 + x_m / m_lon)
    }
}

fn io_err<E: Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}

fn dado_invalido<E: Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

/// Nome do tile Copernicus que cobre `(lat, lon)`, pelo canto SW (ex.: S20_00_W044_00).
pub fn nome_tile(lat: f64, lon: f64) -> String {
    let la = lat.floor() as i32;
    let lo = lon.floor() as i32;
    let ns = if la >= 0 { 'N' } else { 'S' };
    let ew = if lo >= 0 { 'E' } else { 'W' };
    format!("Copernicus_DSM_COG_10_{ns}{:02}_00_{ew}{:03}_00_DEM", la.abs(), lo.abs())
}

/// Baixa o tile que cobre `(lat, lon)` para `cache_root/<id>/raw/copernicus/`.
/// Devolve (caminho, veio_do_cache). Se já existe, não baixa de novo.
pub fn baixar_dem(cache_root: &Path, id: &str, lat: f64, lon: f64) -> io::Result<(PathBuf, bool)> {
    let nome = nome_tile(lat, lon);
    let dir = cache_root.join(id).join("raw").join("copernicus");
    fs::create_dir_all(&dir)?;
    let destino = dir.join(format!("{nome}.tif"));
    if destino.exists() {
        return Ok((destino, true));
    }
    let url = format!("{COPERNICUS_BASE}/{nome}/{nome}.tif");
    let resp = ureq::get(&url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(io_err)?;
    let parte = dir.join(format!("{nome}.tif.part"));
    {
        let mut corpo = resp.into_reader();
        let mut arq = fs::File::create(&parte)?;
        io::copy(&mut corpo, &mut arq)?;
        arq.sync_all()?;
    }
    fs::rename(&parte, &destino)?;
    Ok((destino, false))
}

/// Lê o GeoTIFF (primeiro IFD, float32) e aplica suavização gaussiana (sigma 1 px,
/// raio 2 px), para tirar picos de copas e telhados do DSM.
pub fn ler_dem_suavizado(path: &Path) -> io::Result<Raster> {
    let arq = fs::File::open(path)?;
    let mut dec = Decoder::new(io::BufReader::new(arq)).map_err(dado_invalido)?;
    let (w, h) = dec.dimensions().map_err(dado_invalido)?;
    let escala = dec
        .get_tag_f64_vec(Tag::Unknown(TAG_PIXEL_SCALE))
        .map_err(dado_invalido)?;
    let tie = dec
        .get_tag_f64_vec(Tag::Unknown(TAG_TIEPOINT))
        .map_err(dado_invalido)?;
    if escala.len() < 2 || tie.len() < 6 || tie[0] != 0.0 || tie[1] != 0.0 {
        return Err(dado_invalido("GeoTIFF sem georreferência simples (ModelPixelScale/Tiepoint)"));
    }
    let bruto = match dec.read_image().map_err(dado_invalido)? {
        DecodingResult::F32(v) => v,
        _ => return Err(dado_invalido("DEM esperado em float32")),
    };
    let (w, h) = (w as usize, h as usize);
    if bruto.len() != w * h {
        return Err(dado_invalido("tamanho do DEM não bate com as dimensões"));
    }
    let limpo: Vec<f32> = bruto
        .into_iter()
        .map(|v| if v.is_finite() && v > NODATA_LIMITE { v } else { f32::NAN })
        .collect();
    let valores = suavizar(&limpo, w, h);
    Ok(Raster {
        largura: w,
        altura: h,
        lon0: tie[3],
        lat0: tie[4],
        passo_lon: escala[0],
        passo_lat: escala[1],
        valores,
    })
}

/// Gaussiana separável 5 taps (sigma 1), ignorando NaN e normalizando pelos pesos válidos.
fn suavizar(src: &[f32], w: usize, h: usize) -> Vec<f32> {
    let sigma: f32 = 1.0;
    let nucleo: [f32; 5] = {
        let mut k = [0.0f32; 5];
        for (i, v) in k.iter_mut().enumerate() {
            let d = i as f32 - 2.0;
            *v = (-(d * d) / (2.0 * sigma * sigma)).exp();
        }
        let soma: f32 = k.iter().sum();
        k.map(|v| v / soma)
    };
    let horiz = passa(src, w, h, 1, 0, &nucleo);
    passa(&horiz, w, h, 0, 1, &nucleo)
}

fn passa(src: &[f32], w: usize, h: usize, dx: isize, dy: isize, k: &[f32; 5]) -> Vec<f32> {
    let mut out = vec![f32::NAN; w * h];
    for r in 0..h {
        for c in 0..w {
            if src[r * w + c].is_nan() {
                continue;
            }
            let (mut acc, mut peso) = (0.0f32, 0.0f32);
            for (i, &kw) in k.iter().enumerate() {
                let d = i as isize - 2;
                let rr = r as isize + dy * d;
                let cc = c as isize + dx * d;
                if rr < 0 || cc < 0 || rr >= h as isize || cc >= w as isize {
                    continue;
                }
                let v = src[rr as usize * w + cc as usize];
                if !v.is_nan() {
                    acc += kw * v;
                    peso += kw;
                }
            }
            if peso > 0.0 {
                out[r * w + c] = acc / peso;
            }
        }
    }
    out
}

/// Interpolação bilinear da altura em `(lat, lon)`. `None` fora do raster ou sem dado.
pub fn altura_em(r: &Raster, lat: f64, lon: f64) -> Option<f32> {
    let fc = (lon - r.lon0) / r.passo_lon - 0.5;
    let fr = (r.lat0 - lat) / r.passo_lat - 0.5;
    let c0 = fc.floor();
    let r0 = fr.floor();
    let tx = (fc - c0) as f32;
    let ty = (fr - r0) as f32;
    let (c0, r0) = (c0 as isize, r0 as isize);
    let (mut acc, mut peso) = (0.0f32, 0.0f32);
    for (dr, dc, pw) in [
        (0, 0, (1.0 - tx) * (1.0 - ty)),
        (0, 1, tx * (1.0 - ty)),
        (1, 0, (1.0 - tx) * ty),
        (1, 1, tx * ty),
    ] {
        let rr = r0 + dr;
        let cc = c0 + dc;
        if rr < 0 || cc < 0 || rr >= r.altura as isize || cc >= r.largura as isize {
            continue;
        }
        let v = r.valores[rr as usize * r.largura + cc as usize];
        if !v.is_nan() && pw > 0.0 {
            acc += pw * v;
            peso += pw;
        }
    }
    if peso > 0.0 {
        Some(acc / peso)
    } else {
        None
    }
}

/// Amostra a grade de alturas do tile `(x, y)` (ver doc do módulo).
pub fn alturas_do_tile(r: &Raster, grade: &Grade, x: i32, y: i32) -> io::Result<HeightGrid> {
    let n = (grade.tile_m / grade.celula_m).round() as u32;
    let cols = n + 1;
    let x0 = x as f64 * grade.tile_m;
    let y0 = y as f64 * grade.tile_m;
    let mut values = Vec::with_capacity((cols * cols) as usize);
    for i in 0..cols {
        for j in 0..cols {
            let x_m = x0 + j as f64 * grade.celula_m;
            let y_m = y0 + i as f64 * grade.celula_m;
            let (lat, lon) = grade.latlon(x_m, y_m);
            let h = altura_em(r, lat, lon).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("sem dado de relevo em tile ({x},{y}) ponto ({x_m:.1},{y_m:.1})"),
                )
            })?;
            values.push(h);
        }
    }
    Ok(HeightGrid {
        cols,
        rows: cols,
        values,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nome_do_tile_da_casa() {
        let n = nome_tile(-19.9006792, -43.9174041);
        assert_eq!(n, "Copernicus_DSM_COG_10_S20_00_W044_00_DEM");
    }

    #[test]
    fn nome_do_tile_norte_leste() {
        assert_eq!(
            nome_tile(10.5, 5.2),
            "Copernicus_DSM_COG_10_N10_00_E005_00_DEM"
        );
    }

    /// Raster 4x4 com `valor = col * 1 + row * 10`, pixels de 0.01 grau.
    fn rampa() -> Raster {
        let (w, h) = (4usize, 4usize);
        let valores = (0..h)
            .flat_map(|r| (0..w).map(move |c| c as f32 + 10.0 * r as f32))
            .collect();
        Raster {
            largura: w,
            altura: h,
            lon0: 0.0,
            lat0: 0.0,
            passo_lon: 0.01,
            passo_lat: 0.01,
            valores,
        }
    }

    #[test]
    fn bilinear_em_centro_de_pixel_e_no_meio() {
        let r = rampa();
        // centro do pixel (c=1, r=2): lon=0.015, lat=-0.025 -> valor 1 + 20 = 21
        let v = altura_em(&r, -0.025, 0.015).unwrap();
        assert!((v - 21.0).abs() < 1e-4, "{v}");
        // meio entre (1,2) e (2,2): 21.5
        let v = altura_em(&r, -0.025, 0.02).unwrap();
        assert!((v - 21.5).abs() < 1e-4, "{v}");
        // fora do raster
        assert!(altura_em(&r, 1.0, 1.0).is_none());
    }

    #[test]
    fn tile_com_min_max_esperados() {
        // DEM plano inclinado: h(lon, lat) = 1000 + 2*(lon/passo) + 3*(-lat/passo),
        // em coordenadas de pixel. Bilinear reproduz plano exatamente.
        // Tile 20 m com grade de 10 m: 3x3 amostras, cantos em x,y in {0,10,20} m.
        let grade = Grade {
            lat0: 0.0,
            lon0: 0.0,
            tile_m: 20.0,
            celula_m: 10.0,
        };
        let (w, h) = (400usize, 400usize);
        let passo = 0.0001;
        let (lon0, lat0) = (-0.01, 0.01); // raster cobre o ponto (0,0) com folga
        let valores: Vec<f32> = (0..h)
            .flat_map(|r| {
                (0..w).map(move |c| {
                    // centro do pixel: (lon - lon0)/passo = c+0.5, (lat0 - lat)/passo = r+0.5
                    let x = c as f64 + 0.5;
                    let y = r as f64 + 0.5;
                    (1000.0 + 2.0 * x + 3.0 * y) as f32
                })
            })
            .collect();
        let r = Raster {
            largura: w,
            altura: h,
            lon0,
            lat0,
            passo_lon: passo,
            passo_lat: passo,
            valores,
        };
        let g = alturas_do_tile(&r, &grade, 0, 0).unwrap();
        assert_eq!((g.cols, g.rows), (3, 3));
        // Esperado: mínimo em (x=0, y=+20 m) e máximo em (x=+20 m, y=0).
        let d = (20.0 / RAIO_TERRA_M.to_radians()) / passo; // 20 m em passos de pixel
        let min_esperado = 1000.0 + 2.0 * 100.0 + 3.0 * (100.0 - d);
        let max_esperado = 1000.0 + 2.0 * (100.0 + d) + 3.0 * 100.0;
        let min = g.values.iter().cloned().fold(f32::INFINITY, f32::min) as f64;
        let max = g.values.iter().cloned().fold(f32::NEG_INFINITY, f32::max) as f64;
        assert!((min - min_esperado).abs() < 0.01, "min {min} vs {min_esperado}");
        assert!((max - max_esperado).abs() < 0.01, "max {max} vs {max_esperado}");
        // Norte fica em row maior: valor do canto (row=2, col=0) = mínimo.
        assert!((g.values[2 * 3] as f64 - min_esperado).abs() < 0.01);
    }

    #[test]
    fn suavizacao_reduz_pico_e_preserva_soma() {
        let (w, h) = (9usize, 9usize);
        let mut src = vec![0.0f32; w * h];
        src[4 * w + 4] = 100.0;
        let out = suavizar(&src, w, h);
        let pico = out[4 * w + 4];
        // pico = (w0 * w0) * 100, com w0 = e^0 / soma(1 + 2e^-0.5 + 2e^-2)
        let soma = 1.0 + 2.0 * (-0.5f32).exp() + 2.0 * (-2.0f32).exp();
        let esperado = (1.0 / soma) * (1.0 / soma) * 100.0;
        assert!((pico - esperado).abs() < 1e-3, "{pico} vs {esperado}");
        let total: f32 = out.iter().sum();
        assert!((total - 100.0).abs() < 1e-2, "{total}");
    }

    #[test]
    fn nodata_nao_contamina_vizinhos() {
        let (w, h) = (5usize, 1usize);
        let src = vec![10.0, f32::NAN, 10.0, 10.0, 10.0];
        let out = suavizar(&src, w, h);
        assert!(out[0].is_finite());
        assert!((out[0] - 10.0).abs() < 1e-4);
        assert!((out[2] - 10.0).abs() < 1e-4);
    }
}
