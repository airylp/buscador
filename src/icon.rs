//! Ícono de la aplicación: una «B» pixel-art azul sobre un cuadrado
//! redondeado oscuro y liso (con bordes suavizados), generado en código, sin
//! archivos de imagen. Lo usan Cmd+Tab y el Dock en macOS, y la barra de
//! tareas en Windows/Linux.

use eframe::egui::IconData;

/// La «B» (8×8 + sombra) se dibuja en una cuadrícula de 15×15 «píxeles»
/// gordos sobre el cuadrado redondeado, centrada (celdas 3–11).
const N: isize = 15;
/// Tamaño del ícono de la ventana: 512 px (nítido en Retina).
const SIZE: usize = 512;

const B: [&str; 8] = [
    "######..", //
    "##...##.", //
    "##...##.", //
    "######..", //
    "##....##", //
    "##....##", //
    "##....##", //
    "#######.", //
];

/// Fondo: degradado vertical oscuro.
const BG_TOP: [f32; 3] = [0x25 as f32, 0x2d as f32, 0x3f as f32];
const BG_BOTTOM: [f32; 3] = [0x0b as f32, 0x0e as f32, 0x16 as f32];
const BLUE: [u8; 3] = [0x2f, 0x8c, 0xff];
const BLUE_LIGHT: [u8; 3] = [0x8f, 0xcf, 0xff];
const BLUE_DARK: [u8; 3] = [0x1d, 0x5f, 0xd6];
const SHADOW: [u8; 3] = [0x06, 0x1c, 0x45];

/// Color de la «B» en la celda (fila, columna) de la cuadrícula, si hay.
fn b_cell(r: isize, c: isize) -> Option<[u8; 3]> {
    let (r0, c0) = (3, 3);
    let on = |r: isize, c: isize| {
        let (dr, dc) = (r - r0 as isize, c - c0 as isize);
        (0..8).contains(&dr) && (0..8).contains(&dc) && B[dr as usize].as_bytes()[dc as usize] == b'#'
    };
    if on(r, c) {
        // Brillo arriba a la izquierda de cada trazo y algo más oscuro abajo.
        let above = !on(r - 1, c);
        let left = !on(r, c - 1);
        let below = !on(r + 1, c);
        Some(if above && left {
            BLUE_LIGHT
        } else if below {
            BLUE_DARK
        } else {
            BLUE
        })
    } else if on(r - 1, c - 1) {
        Some(SHADOW)
    } else {
        None
    }
}

/// RGBA del ícono a `size` px.
fn render(size: usize) -> Vec<u8> {
    let s = size as f32;
    // Márgenes y radio como los íconos de macOS (contenido ≈ 82 % del lienzo).
    let m = s * 0.09;
    let half = (s - 2.0 * m) / 2.0;
    let radius = (s - 2.0 * m) * 0.225;
    let center = s / 2.0;
    let mut rgba = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // Distancia con signo al cuadrado redondeado.
            let qx = (px - center).abs() - (half - radius);
            let qy = (py - center).abs() - (half - radius);
            let out = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - radius;
            let alpha = (0.5 - out).clamp(0.0, 1.0);
            if alpha <= 0.0 {
                continue;
            }
            let t = ((py - m) / (s - 2.0 * m)).clamp(0.0, 1.0);
            let mut col = [0u8; 3];
            for k in 0..3 {
                col[k] = (BG_TOP[k] + (BG_BOTTOM[k] - BG_TOP[k]) * t).round() as u8;
            }
            // Filo claro muy suave en el borde: se distingue sobre fondos oscuros.
            let edge = (-out).clamp(0.0, s * 0.012) / (s * 0.012);
            if edge < 1.0 {
                for c in col.iter_mut() {
                    *c = (*c as f32 + (1.0 - edge) * 28.0).min(255.0) as u8;
                }
            }
            let cell = (s - 2.0 * m) / N as f32;
            let (gr, gc) = (((py - m) / cell).floor() as isize, ((px - m) / cell).floor() as isize);
            if (0..N).contains(&gr) && (0..N).contains(&gc) {
                if let Some(b) = b_cell(gr, gc) {
                    col = b;
                }
            }
            let i = (y * size + x) * 4;
            rgba[i..i + 4].copy_from_slice(&[col[0], col[1], col[2], (alpha * 255.0).round() as u8]);
        }
    }
    rgba
}

pub fn app_icon() -> IconData {
    let size = SIZE;
    IconData { rgba: render(size), width: size as u32, height: size as u32 }
}

/// El ícono como PNG (sin dependencias: deflate sin comprimir).
pub fn png(size: usize) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    let rgba = render(size);
    let mut raw = Vec::with_capacity(size * (size * 4 + 1));
    for row in rgba.chunks(size * 4) {
        raw.push(0); // filtro «ninguno»
        raw.extend_from_slice(row);
    }
    // zlib con bloques «stored».
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push((i + 1 == blocks.len()) as u8);
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(size as u32).to_be_bytes());
    ihdr.extend_from_slice(&(size as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 bits, RGBA
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Escribe en `dir` los PNG que pide `iconutil` para crear el `.icns`.
pub fn write_iconset(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for s in [16, 32, 128, 256, 512] {
        std::fs::write(dir.join(format!("icon_{s}x{s}.png")), png(s))?;
        std::fs::write(dir.join(format!("icon_{s}x{s}@2x.png")), png(s * 2))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_size_and_shape() {
        let ic = super::app_icon();
        assert_eq!((ic.width, ic.height, ic.rgba.len()), (512, 512, 512 * 512 * 4));
        assert_eq!(ic.rgba[3], 0, "esquina transparente");
        let p = super::png(32);
        assert_eq!(&p[..8], b"\x89PNG\r\n\x1a\n");
        // BUSCADOR_ICON=ruta.rgba guarda el ícono para verlo.
        if let Ok(p) = std::env::var("BUSCADOR_ICON") {
            let mut out = Vec::new();
            out.extend_from_slice(&ic.width.to_le_bytes());
            out.extend_from_slice(&ic.height.to_le_bytes());
            out.extend_from_slice(&ic.rgba);
            std::fs::write(p, out).unwrap();
        }
    }
}
