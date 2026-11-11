//! Ícono de la aplicación: una «B» pixel-art, generada en código (sin
//! archivos de imagen). Lo usan Cmd+Tab y el Dock en macOS, y la barra de
//! tareas en Windows/Linux.

use eframe::egui::IconData;

/// Cuadrícula de 16×16 «píxeles» gordos.
const N: usize = 16;
/// Tamaño de cada píxel gordo: 16 × 32 = 512 px (nítido en Retina).
const CELL: usize = 32;

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

const BG: [u8; 3] = [0x1b, 0x24, 0x33];
const BG_TOP: [u8; 3] = [0x26, 0x32, 0x4a];
const BG_BOTTOM: [u8; 3] = [0x12, 0x19, 0x23];
const GREEN: [u8; 3] = [0x3d, 0xdc, 0x6e];
const GREEN_LIGHT: [u8; 3] = [0x8b, 0xf5, 0xa8];
const SHADOW: [u8; 3] = [0x0e, 0x5a, 0x2a];

fn grid() -> [[Option<[u8; 3]>; N]; N] {
    let mut g = [[None; N]; N];
    // Fondo: cuadrado de 14×14 con las esquinas recortadas (redondeo pixelado).
    for (r, row) in g.iter_mut().enumerate().take(15).skip(1) {
        for (c, cell) in row.iter_mut().enumerate().take(15).skip(1) {
            let corner = (r == 1 || r == 14) && (c == 1 || c == 14);
            if !corner {
                *cell = Some(match r {
                    1 => BG_TOP,
                    14 => BG_BOTTOM,
                    _ => BG,
                });
            }
        }
    }
    // La «B», centrada (filas 4–11, columnas 4–11), con sombra abajo a la derecha.
    let (r0, c0) = (4, 4);
    for (dr, line) in B.iter().enumerate() {
        for (dc, ch) in line.chars().enumerate() {
            if ch == '#' {
                g[r0 + dr + 1][c0 + dc + 1] = Some(SHADOW);
            }
        }
    }
    for (dr, line) in B.iter().enumerate() {
        for (dc, ch) in line.chars().enumerate() {
            if ch == '#' {
                // Brillo en el borde superior izquierdo de cada trazo.
                let above = dr == 0 || B[dr - 1].as_bytes()[dc] != b'#';
                let left = dc == 0 || line.as_bytes()[dc - 1] != b'#';
                g[r0 + dr][c0 + dc] = Some(if above && left { GREEN_LIGHT } else { GREEN });
            }
        }
    }
    g
}

/// RGBA del ícono a `size` px (múltiplo de 16 para que los píxeles sean exactos).
fn render(size: usize) -> Vec<u8> {
    let g = grid();
    let mut rgba = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            if let Some([r, gr, b]) = g[y * N / size][x * N / size] {
                let i = (y * size + x) * 4;
                rgba[i..i + 4].copy_from_slice(&[r, gr, b, 255]);
            }
        }
    }
    rgba
}

pub fn app_icon() -> IconData {
    let size = N * CELL;
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
