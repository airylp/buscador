//! Detección de encoding y utilidades relacionadas.

use encoding_rs::*;

/// Cómo se representa el salto de línea en el archivo.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Newline {
    /// `\n` es un único byte 0x0A (UTF-8 y todos los encodings compatibles con ASCII).
    Byte,
    /// `\n` es 0x0A 0x00 alineado a 2 bytes.
    Utf16Le,
    /// `\n` es 0x00 0x0A alineado a 2 bytes.
    Utf16Be,
}

impl Newline {
    pub fn for_encoding(enc: &'static Encoding) -> Self {
        if enc == UTF_16LE {
            Newline::Utf16Le
        } else if enc == UTF_16BE {
            Newline::Utf16Be
        } else {
            Newline::Byte
        }
    }

    pub fn unit(self) -> usize {
        match self {
            Newline::Byte => 1,
            _ => 2,
        }
    }
}

/// Encodings ofrecidos en el selector. Todos son "seguros" para buscar `\n`
/// byte a byte (ningún byte de continuación vale 0x0A), salvo UTF-16 que se
/// trata aparte.
pub fn available() -> Vec<&'static Encoding> {
    vec![
        UTF_8,
        UTF_16LE,
        UTF_16BE,
        WINDOWS_1252, // también cubre ISO-8859-1 / Latin-1
        ISO_8859_15,
        WINDOWS_1250,
        ISO_8859_2,
        WINDOWS_1251,
        KOI8_R,
        IBM866,
        WINDOWS_1253,
        WINDOWS_1254,
        WINDOWS_1256,
        SHIFT_JIS,
        EUC_JP,
        GBK,
        GB18030,
        BIG5,
        EUC_KR,
    ]
}

/// Nombre amigable para mostrar.
pub fn label(enc: &'static Encoding) -> String {
    if enc == WINDOWS_1252 {
        "windows-1252 / Latin-1".to_string()
    } else {
        enc.name().to_string()
    }
}

/// Detecta el encoding mirando el BOM y una muestra del inicio del archivo.
/// Devuelve el encoding y el motivo de la elección.
pub fn detect(data: &[u8]) -> (&'static Encoding, &'static str) {
    if let Some((enc, _)) = Encoding::for_bom(data) {
        return (enc, "BOM");
    }
    let sample = &data[..data.len().min(1 << 20)];
    if sample.is_empty() {
        return (UTF_8, "vacío");
    }

    // UTF-16 sin BOM: muchos bytes 0 en posiciones pares o impares.
    if sample.len() >= 4 {
        let (mut zero_even, mut zero_odd) = (0usize, 0usize);
        for (i, b) in sample.iter().enumerate() {
            if *b == 0 {
                if i % 2 == 0 {
                    zero_even += 1;
                } else {
                    zero_odd += 1;
                }
            }
        }
        let half = sample.len() / 2;
        if zero_odd > half * 3 / 10 && zero_even < half / 20 {
            return (UTF_16LE, "heurística");
        }
        if zero_even > half * 3 / 10 && zero_odd < half / 20 {
            return (UTF_16BE, "heurística");
        }
    }

    // UTF-8: tolera algunos bytes inválidos (logs con basura ocasional).
    let (errors, multibyte) = utf8_stats(sample);
    if errors == 0 || multibyte > errors * 4 {
        return (UTF_8, if errors == 0 { "UTF-8 válido" } else { "UTF-8 (con errores)" });
    }

    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    det.feed(sample, data.len() == sample.len());
    (det.guess(None, chardetng::Utf8Detection::Deny), "chardetng")
}

/// Cuenta secuencias UTF-8 inválidas y caracteres multibyte válidos.
fn utf8_stats(mut s: &[u8]) -> (usize, usize) {
    let (mut errors, mut multibyte) = (0usize, 0usize);
    loop {
        match std::str::from_utf8(s) {
            Ok(v) => {
                multibyte += v.chars().filter(|c| !c.is_ascii()).count();
                return (errors, multibyte);
            }
            Err(e) => {
                let valid = e.valid_up_to();
                // SAFETY: from_utf8 garantiza que el prefijo es válido.
                let v = unsafe { std::str::from_utf8_unchecked(&s[..valid]) };
                multibyte += v.chars().filter(|c| !c.is_ascii()).count();
                match e.error_len() {
                    Some(n) => {
                        errors += 1;
                        s = &s[valid + n..];
                    }
                    // Corte a mitad de carácter al final de la muestra.
                    None => return (errors, multibyte),
                }
            }
        }
    }
}

/// Codifica `s` en `enc` y lo añade a `out`. Si algún carácter no se puede
/// representar devuelve `Err(c)` (sin usar el reemplazo `&#…;` de encoding_rs)
/// y `out` queda sin cambios.
pub fn encode_into(enc: &'static Encoding, s: &str, out: &mut Vec<u8>) -> Result<(), char> {
    if enc == UTF_8 {
        out.extend_from_slice(s.as_bytes());
        return Ok(());
    }
    if enc == UTF_16LE || enc == UTF_16BE {
        for u in s.encode_utf16() {
            out.extend_from_slice(&if enc == UTF_16LE { u.to_le_bytes() } else { u.to_be_bytes() });
        }
        return Ok(());
    }
    let start = out.len();
    let mut encoder = enc.new_encoder();
    let mut src = s;
    loop {
        let need = encoder.max_buffer_length_from_utf8_without_replacement(src.len()).unwrap_or(src.len() * 4 + 16);
        let at = out.len();
        out.resize(at + need, 0);
        let (res, read, written) = encoder.encode_from_utf8_without_replacement(src, &mut out[at..], true);
        out.truncate(at + written);
        match res {
            EncoderResult::InputEmpty => return Ok(()),
            EncoderResult::OutputFull => src = &src[read..],
            EncoderResult::Unmappable(c) => {
                out.truncate(start);
                return Err(c);
            }
        }
    }
}

/// Como [`encode_into`], pero sustituye lo no representable por `?`.
/// Devuelve cuántos caracteres se sustituyeron.
pub fn encode_lossy_into(enc: &'static Encoding, s: &str, out: &mut Vec<u8>) -> usize {
    if encode_into(enc, s, out).is_ok() {
        return 0;
    }
    let mut bad = 0;
    let mut buf = [0u8; 4];
    for c in s.chars() {
        if encode_into(enc, c.encode_utf8(&mut buf), out).is_err() {
            bad += 1;
            let _ = encode_into(enc, "?", out);
        }
    }
    bad
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_strict_and_lossy() {
        let mut v = Vec::new();
        encode_into(WINDOWS_1252, "año", &mut v).unwrap();
        assert_eq!(v, b"a\xF1o");
        assert_eq!(encode_into(WINDOWS_1252, "日本", &mut v), Err('日'));
        assert_eq!(v, b"a\xF1o");
        v.clear();
        encode_into(UTF_16BE, "ñ\n", &mut v).unwrap();
        assert_eq!(v, [0x00, 0xF1, 0x00, 0x0A]);
        v.clear();
        assert_eq!(encode_lossy_into(SHIFT_JIS, "a€b", &mut v), 1);
        assert_eq!(v, b"a?b");
    }
}
