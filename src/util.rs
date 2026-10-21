//! Petites fonctions : chemins POSIX, tailles, dates, droits, tri naturel.

use chrono::{Local, TimeZone};
use std::cmp::Ordering;

pub fn human(size: u64) -> String {
    let units = ["o", "Kio", "Mio", "Gio", "Tio"];
    let mut s = size as f64;
    for (i, u) in units.iter().enumerate() {
        if s < 1024.0 || i == units.len() - 1 {
            return if i == 0 { format!("{size} {u}") } else { format!("{s:.1} {u}") };
        }
        s /= 1024.0;
    }
    unreachable!()
}

pub fn fmt_time(mtime: f64, fmt: &str) -> String {
    match Local.timestamp_opt(mtime.floor() as i64, 0) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => t.format(fmt).to_string(),
        _ => String::new(),
    }
}

pub fn now_hms() -> String {
    Local::now().format("%H:%M:%S").to_string()
}

/// Équivalent de `stat.filemode`.
pub fn filemode(mode: u32) -> String {
    let ft = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        0o010000 => 'p',
        0o140000 => 's',
        0o060000 => 'b',
        0o020000 => 'c',
        0o100000 => '-',
        _ => '?',
    };
    let mut s = String::with_capacity(10);
    s.push(ft);
    let bits = [
        (0o400, 'r'), (0o200, 'w'), (0o100, 'x'),
        (0o040, 'r'), (0o020, 'w'), (0o010, 'x'),
        (0o004, 'r'), (0o002, 'w'), (0o001, 'x'),
    ];
    for (i, (b, c)) in bits.iter().enumerate() {
        let mut ch = if mode & b != 0 { *c } else { '-' };
        if i == 2 && mode & 0o4000 != 0 {
            ch = if mode & 0o100 != 0 { 's' } else { 'S' };
        }
        if i == 5 && mode & 0o2000 != 0 {
            ch = if mode & 0o010 != 0 { 's' } else { 'S' };
        }
        if i == 8 && mode & 0o1000 != 0 {
            ch = if mode & 0o001 != 0 { 't' } else { 'T' };
        }
        s.push(ch);
    }
    s
}

// ---------------------------------------------------------------- chemins POSIX

pub fn basename(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

pub fn dirname(p: &str) -> String {
    match p.rfind('/') {
        Some(i) => {
            let head = &p[..i + 1];
            let t = head.trim_end_matches('/');
            if t.is_empty() { "/".to_string() } else { t.to_string() }
        }
        None => String::new(),
    }
}

pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') || a.is_empty() {
        b.to_string()
    } else if a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

pub fn normpath(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let abs = p.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for comp in p.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if !out.is_empty() && *out.last().unwrap() != ".." {
                    out.pop();
                } else if !abs {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    let body = out.join("/");
    if abs {
        format!("/{body}")
    } else if body.is_empty() {
        ".".into()
    } else {
        body
    }
}

/// `posixpath.splitext` : (racine, extension avec le point).
pub fn splitext(p: &str) -> (String, String) {
    let sep = p.rfind('/').map(|i| i + 1).unwrap_or(0);
    let name = &p[sep..];
    if let Some(dot) = name.rfind('.') {
        let lead = name[..dot].chars().all(|c| c == '.');
        if !lead {
            return (p[..sep + dot].to_string(), name[dot..].to_string());
        }
    }
    (p.to_string(), String::new())
}

/// Extension pour couleurs/icônes : après le dernier `.`, minuscules.
pub fn ext_of(name: &str) -> String {
    let t = name.trim_matches('.');
    match t.rfind('.') {
        Some(i) => t[i + 1..].to_lowercase(),
        None => String::new(),
    }
}

pub fn relpath(path: &str, start: &str) -> String {
    let p: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let s: Vec<&str> = start.split('/').filter(|s| !s.is_empty()).collect();
    let common = p.iter().zip(s.iter()).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = std::iter::repeat_n("..", s.len() - common).collect();
    parts.extend(&p[common..]);
    if parts.is_empty() { ".".into() } else { parts.join("/") }
}

pub fn is_within(path: &str, dir: &str) -> bool {
    path == dir || path.starts_with(&format!("{}/", dir.trim_end_matches('/')))
}

// ---------------------------------------------------------------- tri naturel

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NatPart {
    Num(u128, usize),
    Text(String),
}

pub fn natural_key(name: &str) -> Vec<NatPart> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut digits = false;
    for c in name.chars() {
        let d = c.is_ascii_digit();
        if !cur.is_empty() && d != digits {
            out.push(make_part(&cur, digits));
            cur.clear();
        }
        digits = d;
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(make_part(&cur, digits));
    }
    out
}

fn make_part(s: &str, digits: bool) -> NatPart {
    if digits {
        NatPart::Num(s.parse().unwrap_or(u128::MAX), s.len())
    } else {
        NatPart::Text(s.to_lowercase())
    }
}

pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (ka, kb) = (natural_key(a), natural_key(b));
    for (x, y) in ka.iter().zip(kb.iter()) {
        let o = match (x, y) {
            (NatPart::Num(m, _), NatPart::Num(n, _)) => m.cmp(n),
            (NatPart::Text(m), NatPart::Text(n)) => m.cmp(n),
            // Python comparerait int et str (erreur) ; on met les nombres d'abord
            (NatPart::Num(..), NatPart::Text(_)) => Ordering::Less,
            (NatPart::Text(_), NatPart::Num(..)) => Ordering::Greater,
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    ka.len().cmp(&kb.len())
}

// ---------------------------------------------------------------- divers

pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    }
}

pub fn shell_join<S: AsRef<str>>(args: &[S]) -> String {
    args.iter().map(|a| shell_quote(a.as_ref())).collect::<Vec<_>>().join(" ")
}

pub fn sha1_hex(data: &[u8]) -> String {
    use sha1::Digest;
    let mut h = sha1::Sha1::new();
    h.update(data);
    hex(&h.finalize())
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(data);
    hex(&h.finalize())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Encodage URL (comme `urllib.parse.quote`, `/` non encodé).
pub fn url_quote(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn url_unquote(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Tronque à `width` colonnes avec `…`.
pub fn truncate(s: &str, width: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    if s.width() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > width - 1 {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

pub fn pad(s: &str, width: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    let w = s.width();
    if w >= width { s.to_string() } else { format!("{s}{}", " ".repeat(width - w)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn t() {
        assert_eq!(human(512), "512 o");
        assert_eq!(human(1536), "1.5 Kio");
        assert_eq!(splitext("/a/.bashrc").1, "");
        assert_eq!(splitext("/a/x.tar.gz"), ("/a/x.tar".into(), ".gz".into()));
        assert_eq!(normpath("/a/b/../c/./"), "/a/c");
        assert_eq!(dirname("/a"), "/");
        assert_eq!(dirname("/a/b"), "/a");
        assert_eq!(filemode(0o40755), "drwxr-xr-x");
        assert_eq!(relpath("/a/b/c", "/a"), "b/c");
        assert_eq!(url_unquote("a%20b"), "a b");
        assert_eq!(ext_of(".bashrc"), "");
        assert!(natural_cmp("a2", "a10").is_lt());
    }
}
