//! Fechas y horas al principio de las líneas de log, para «ir a fecha/hora».
//!
//! Reconoce los formatos más comunes (ISO 8601 / `AAAA-MM-DD HH:MM:SS`,
//! `DD/MM/AAAA`, Apache `21/Nov/2026:10:22:33`, syslog `Nov 21 10:22:33`,
//! solo hora `10:22:33`, segundos o milisegundos desde 1970) y busca por
//! bisección la primera línea con una marca >= la pedida: con millones de
//! líneas se leen unas pocas decenas.

/// Momento: segundos desde una época cualquiera + nanosegundos. Lo que falta
/// (año, fecha) se deja a 0 y se compara igual en ambos lados (ver `Scope`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ts {
    pub secs: i64,
    pub nanos: u32,
}

/// Qué traen las marcas de un archivo (o lo que se pide).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// Solo hora del día.
    Time,
    /// Mes, día y hora (syslog: sin año).
    NoYear,
    /// Fecha y hora completas.
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stamp {
    pub scope: Scope,
    pub year: i64,
    pub month: u32,
    pub day: u32,
    /// Segundos del día y nanosegundos.
    pub sod: u32,
    pub nanos: u32,
}

impl Stamp {
    /// Valor comparable con lo que hay en `scope` (lo demás se ignora).
    pub fn key(&self, scope: Scope) -> Ts {
        let days = match scope {
            Scope::Time => 0,
            Scope::NoYear => days_from_civil(2000, self.month, self.day),
            Scope::Full => days_from_civil(self.year, self.month, self.day),
        };
        Ts { secs: days * 86_400 + self.sod as i64, nanos: self.nanos }
    }

    pub fn show(&self) -> String {
        let (h, m, s) = (self.sod / 3600, self.sod / 60 % 60, self.sod % 60);
        match self.scope {
            Scope::Time => format!("{h:02}:{m:02}:{s:02}"),
            Scope::NoYear => format!("{:02}-{:02} {h:02}:{m:02}:{s:02}", self.month, self.day),
            Scope::Full => format!("{}-{:02}-{:02} {h:02}:{m:02}:{s:02}", self.year, self.month, self.day),
        }
    }
}

/// Días desde 1970-01-01 (algoritmo de H. Hinnant).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
/// Abreviaturas en español y francés que difieren de las inglesas.
const MONTHS_ALT: [(&str, u32); 12] = [
    ("ene", 1),
    ("abr", 4),
    ("ago", 8),
    ("dic", 12),
    ("janv", 1),
    ("fév", 2),
    ("fev", 2),
    ("avr", 4),
    ("mai", 5),
    ("juil", 7),
    ("aoû", 8),
    ("déc", 12),
];

fn month(w: &str) -> Option<u32> {
    let w = w.to_lowercase();
    if w.len() < 3 {
        return None;
    }
    if let Some(i) = MONTHS.iter().position(|m| w.starts_with(m)) {
        return Some(i as u32 + 1);
    }
    MONTHS_ALT.iter().find(|(m, _)| w.starts_with(m)).map(|(_, n)| *n)
}

/// Lee `k` dígitos en `i`.
fn num(b: &[u8], i: usize, k: usize) -> Option<u32> {
    let s = b.get(i..i + k)?;
    if !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(s.iter().fold(0, |a, &c| a * 10 + (c - b'0') as u32))
}

/// `HH:MM[:SS[.fff]]` en `i`: (segundos del día, nanos, bytes leídos).
fn time_at(b: &[u8], i: usize) -> Option<(u32, u32, usize)> {
    let h = num(b, i, 2).or_else(|| num(b, i, 1).filter(|_| b.get(i + 1) == Some(&b':')))?;
    let hl = if h >= 10 || b.get(i + 1) != Some(&b':') { 2 } else { 1 };
    let mut j = i + hl;
    if b.get(j) != Some(&b':') || h > 23 {
        return None;
    }
    let m = num(b, j + 1, 2).filter(|m| *m < 60)?;
    j += 3;
    let mut s = 0;
    let mut nanos = 0;
    if b.get(j) == Some(&b':') {
        if let Some(v) = num(b, j + 1, 2).filter(|v| *v < 61) {
            s = v.min(59);
            j += 3;
            if matches!(b.get(j), Some(b'.' | b',')) && b.get(j + 1).is_some_and(u8::is_ascii_digit) {
                let mut k = j + 1;
                let mut scale = 100_000_000u32;
                while k < b.len() && b[k].is_ascii_digit() {
                    nanos += (b[k] - b'0') as u32 * scale;
                    scale /= 10;
                    k += 1;
                }
                j = k;
            }
        }
    }
    // No es una hora si sigue pegado un dígito.
    if b.get(j).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    Some((h * 3600 + m * 60 + s, nanos, j - i))
}

fn valid(y: i64, m: u32, d: u32) -> bool {
    (1..=12).contains(&m) && (1..=31).contains(&d) && (1900..=2200).contains(&y)
}

/// Marca en la posición `i` de `b` (que empieza en un límite de palabra).
fn stamp_at(s: &str, b: &[u8], i: usize) -> Option<Stamp> {
    let full = |y: i64, mo: u32, d: u32, rest: usize| -> Option<Stamp> {
        if !valid(y, mo, d) {
            return None;
        }
        // Separador entre fecha y hora: espacio, «T» o «:» (Apache).
        let t = matches!(b.get(rest), Some(b' ' | b'T' | b':' | b'_')).then(|| time_at(b, rest + 1)).flatten();
        let (sod, nanos) = t.map_or((0, 0), |t| (t.0, t.1));
        Some(Stamp { scope: Scope::Full, year: y, month: mo, day: d, sod, nanos })
    };
    // AAAA-MM-DD, AAAA/MM/DD, AAAA.MM.DD
    if let (Some(y), Some(sep)) = (num(b, i, 4), b.get(i + 4)) {
        if matches!(sep, b'-' | b'/' | b'.') && b.get(i + 7) == Some(sep) {
            if let (Some(mo), Some(d)) = (num(b, i + 5, 2), num(b, i + 8, 2)) {
                return full(y as i64, mo, d, i + 10);
            }
        }
        // AAAAMMDD[T]HHMMSS (compacto)
        if let (Some(mo), Some(d)) = (num(b, i + 4, 2), num(b, i + 6, 2)) {
            if b.get(i + 8) == Some(&b'T') {
                if let (Some(h), Some(mi), Some(se)) = (num(b, i + 9, 2), num(b, i + 11, 2), num(b, i + 13, 2)) {
                    if valid(y as i64, mo, d) && h < 24 && mi < 60 && se < 61 {
                        return Some(Stamp { scope: Scope::Full, year: y as i64, month: mo, day: d, sod: h * 3600 + mi * 60 + se.min(59), nanos: 0 });
                    }
                }
            }
        }
    }
    // DD/MM/AAAA, DD-MM-AAAA, DD.MM.AAAA (orden europeo)
    if let (Some(d), Some(sep)) = (num(b, i, 2), b.get(i + 2)) {
        if matches!(sep, b'/' | b'-' | b'.') {
            if let (Some(mo), Some(y)) = (num(b, i + 3, 2).filter(|_| b.get(i + 5) == Some(sep)), num(b, i + 6, 4)) {
                return full(y as i64, mo, d, i + 10);
            }
            // DD/Mon/AAAA (Apache)
            let end = s[i + 3..].find(|c: char| !c.is_alphabetic()).map(|e| i + 3 + e)?;
            if let Some(mo) = month(&s[i + 3..end]) {
                if b.get(end) == Some(sep) {
                    if let Some(y) = num(b, end + 1, 4) {
                        return full(y as i64, mo, d, end + 5);
                    }
                }
            }
        }
    }
    // Mon DD HH:MM:SS (syslog), opcionalmente con el año después.
    if b[i].is_ascii_alphabetic() {
        let end = s[i..].find(|c: char| !c.is_alphabetic()).map_or(s.len(), |e| i + e);
        if end - i <= 9 {
            if let Some(mo) = month(&s[i..end]) {
                let mut j = end;
                while b.get(j) == Some(&b' ') {
                    j += 1;
                }
                let dl = if num(b, j, 2).is_some() { 2 } else { 1 };
                let d = num(b, j, dl).filter(|d| (1..=31).contains(d))?;
                j += dl;
                if b.get(j) == Some(&b',') {
                    j += 1;
                }
                if b.get(j) != Some(&b' ') {
                    return None;
                }
                // «Nov 21 2026 10:00:00» o «Nov 21, 2026»
                if let Some(y) = num(b, j + 1, 4).filter(|_| !matches!(b.get(j + 3), Some(b':'))) {
                    return full(y as i64, mo, d, j + 5);
                }
                let (sod, nanos, _) = time_at(b, j + 1)?;
                return Some(Stamp { scope: Scope::NoYear, year: 0, month: mo, day: d, sod, nanos });
            }
        }
    }
    None
}

/// Primera marca de fecha/hora en los primeros bytes de una línea.
pub fn find_in_line(line: &str) -> Option<Stamp> {
    let lim = crate::buffer::floor_char(line, 120);
    let s = &line[..lim];
    let b = s.as_bytes();
    // Segundos o milisegundos desde 1970 al principio de la línea.
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if (digits == 10 || digits == 13) && !matches!(b.get(digits), Some(b'-' | b'/' | b':' | b'.')) {
        let v: i64 = s[..digits].parse().ok()?;
        let (secs, nanos) = if digits == 13 { (v / 1000, (v % 1000) as u32 * 1_000_000) } else { (v, 0) };
        let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
        if (2000..=2100).contains(&y) {
            return Some(Stamp { scope: Scope::Full, year: y, month: m, day: d, sod: secs.rem_euclid(86_400) as u32, nanos });
        }
    }
    let mut time_only = None;
    for (i, c) in s.char_indices() {
        if i > 0 && (b[i - 1].is_ascii_alphanumeric()) {
            continue;
        }
        if !c.is_ascii_alphanumeric() {
            continue;
        }
        if let Some(st) = stamp_at(s, b, i) {
            return Some(st);
        }
        if time_only.is_none() && c.is_ascii_digit() {
            if let Some((sod, nanos, _)) = time_at(b, i) {
                time_only = Some(Stamp { scope: Scope::Time, year: 0, month: 0, day: 0, sod, nanos });
            }
        }
    }
    time_only
}

/// Lo que escribe el usuario: «2026-11-21 10:30», «21/11/2026», «10:30:15»,
/// «Nov 21 10:30». `None` si no parece una fecha/hora (p. ej. un nº de línea).
pub fn parse_query(q: &str) -> Option<Stamp> {
    let q = q.trim();
    if q.is_empty() || q.bytes().all(|c| c.is_ascii_digit() || matches!(c, b'.' | b',' | b'_')) {
        return None;
    }
    let st = find_in_line(q)?;
    Some(st)
}

/// Busca la primera línea con marca >= `target`. `line(n)` da el principio de
/// la línea `n`; `total` = nº de líneas. Devuelve (línea, su marca) o `None`
/// si el archivo no tiene marcas.
pub fn seek(target: &Stamp, total: u64, line: impl Fn(u64) -> String) -> Option<(u64, Stamp)> {
    /// Cuántas líneas seguidas sin marca se miran (trazas de pila, JSON partido…).
    const SCAN: u64 = 2000;
    let stamp_from = |from: u64, until: u64| -> Option<(u64, Stamp)> {
        (from..until.min(from + SCAN)).find_map(|n| find_in_line(&line(n)).map(|s| (n, s)))
    };
    let (first_line, first) = stamp_from(0, total)?;
    // Se compara con lo que traen las líneas del archivo.
    let scope = first.scope.min(target.scope);
    let mut want = *target;
    if target.scope == Scope::Time && first.scope > Scope::Time {
        // Solo hora: el día de la primera línea (o el siguiente si ya pasó esa hora).
        want = Stamp { scope: first.scope, year: first.year, month: first.month, day: first.day, ..*target };
        if want.key(first.scope) < first.key(first.scope) {
            let (y, m, d) = civil_from_days(days_from_civil(first.year.max(2000), first.month, first.day) + 1);
            want = Stamp { year: if first.scope == Scope::Full { y } else { 0 }, month: m, day: d, ..want };
        }
    }
    let scope = if target.scope == Scope::Time { first.scope } else { scope };
    let key = want.key(scope);
    let (mut lo, mut hi) = (first_line, total);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match stamp_from(mid, hi) {
            Some((k, s)) if s.key(scope) < key => lo = k + 1,
            _ => hi = mid,
        }
    }
    stamp_from(lo, total).or_else(|| {
        // Todo es anterior a lo pedido: la última línea con marca.
        let from = total.saturating_sub(SCAN);
        (from..total).rev().find_map(|n| find_in_line(&line(n)).map(|s| (n, s)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(s: &str) -> Stamp {
        find_in_line(s).unwrap_or_else(|| panic!("sin marca: {s}"))
    }

    #[test]
    fn formats() {
        let a = st("2026-11-21T10:22:33.250Z INFO x");
        assert_eq!((a.scope, a.year, a.month, a.day, a.sod, a.nanos), (Scope::Full, 2026, 11, 21, 37353, 250_000_000));
        assert_eq!(st("[2026/11/21 10:22:33] x").sod, 37353);
        assert_eq!(st("21/11/2026 10:22 x").day, 21);
        let ap = st(r#"1.2.3.4 - - [21/Nov/2026:10:22:33 +0000] "GET / HTTP/1.1""#);
        assert_eq!((ap.month, ap.day, ap.sod), (11, 21, 37353));
        let sy = st("Nov  2 10:22:33 host sshd[1]: x");
        assert_eq!((sy.scope, sy.month, sy.day), (Scope::NoYear, 11, 2));
        assert_eq!(st("10:22:33.5 hilo-1 x").scope, Scope::Time);
        let ep = st("1795263753 evento");
        assert_eq!((ep.year, ep.month, ep.day), (2026, 11, 21));
        assert!(find_in_line("versión 1.2.3 sin fecha").is_none());
        assert!(parse_query("1500").is_none(), "un número es una línea");
        assert_eq!(parse_query("10:30").unwrap().scope, Scope::Time);
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
    }

    #[test]
    fn bisect() {
        // Una línea por minuto durante dos días, con trazas sin fecha entre medias.
        let mut lines = Vec::new();
        for m in 0..2 * 24 * 60 {
            let (d, h, mi) = (20 + m / 1440, m / 60 % 24, m % 60);
            lines.push(format!("2026-11-{d:02} {h:02}:{mi:02}:00 INFO evento {m}"));
            if m % 7 == 0 {
                lines.push("    at algo.Clase(Clase.java:1)".into());
            }
        }
        let get = |n: u64| lines[n as usize].clone();
        let total = lines.len() as u64;
        let (n, s) = seek(&parse_query("2026-11-21 10:30").unwrap(), total, get).unwrap();
        assert!(lines[n as usize].starts_with("2026-11-21 10:30:00"), "{}", lines[n as usize]);
        assert_eq!(s.sod, 10 * 3600 + 30 * 60);
        // Solo hora: el primer día.
        let (n, _) = seek(&parse_query("10:30:30").unwrap(), total, get).unwrap();
        assert!(lines[n as usize].starts_with("2026-11-20 10:31:00"));
        // Antes del principio y después del final.
        assert_eq!(seek(&parse_query("2020-01-01").unwrap(), total, get).unwrap().0, 0);
        let (n, _) = seek(&parse_query("2030-01-01").unwrap(), total, get).unwrap();
        assert!(lines[n as usize].starts_with("2026-11-21 23:59"));
        // Sin marcas.
        assert!(seek(&parse_query("10:00").unwrap(), 3, |_| "nada".into()).is_none());
    }
}
