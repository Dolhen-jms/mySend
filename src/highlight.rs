//! Coloration syntaxique (syntect) en couleurs ANSI : le rendu suit la palette du terminal
//! (ou celle du thème, pour l'interface graphique).

use crate::util::ext_of;
use std::sync::OnceLock;
use syntect::highlighting::{
    Color as SColor, FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::SyntaxSet;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fg {
    Default,
    Dim,
    /// Couleur ANSI 0-15.
    Ansi(u8),
}

#[derive(Clone, Debug)]
pub struct Seg {
    pub text: String,
    pub fg: Fg,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

fn syntaxes() -> &'static SyntaxSet {
    static SS: OnceLock<SyntaxSet> = OnceLock::new();
    SS.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Couleur encodée façon bat : a=0 → index ANSI dans r ; a=1 → couleur par défaut ; a=2 → atténué.
fn ac(n: u8) -> SColor {
    SColor { r: n, g: 0, b: 0, a: 0 }
}

fn ansi_theme() -> &'static Theme {
    static TH: OnceLock<Theme> = OnceLock::new();
    TH.get_or_init(|| {
        let item = |sel: &str, col: Option<SColor>, fs: Option<FontStyle>| ThemeItem {
            scope: sel.parse::<ScopeSelectors>().unwrap(),
            style: StyleModifier { foreground: col, background: None, font_style: fs },
        };
        let dimc = SColor { r: 0, g: 0, b: 0, a: 2 };
        Theme {
            name: Some("ansi_dark".into()),
            author: None,
            settings: ThemeSettings {
                foreground: Some(SColor { r: 0, g: 0, b: 0, a: 1 }),
                background: Some(SColor { r: 0, g: 0, b: 0, a: 1 }),
                ..Default::default()
            },
            scopes: vec![
                item("comment, punctuation.definition.comment", Some(dimc), None),
                item("meta.preprocessor, keyword.control.import.c", None, Some(FontStyle::BOLD)),
                item("keyword, storage.modifier, storage.type.function, keyword.declaration", Some(ac(12)), None),
                item("storage.type, support.type, entity.name.type", Some(ac(14)), None),
                item("keyword.operator.word, keyword.operator.logical.python", Some(ac(13)), None),
                item("support.function.builtin", Some(ac(14)), None),
                item("entity.name.function, support.function", Some(ac(10)), None),
                item("entity.name.namespace, entity.name.module", Some(ac(14)), Some(FontStyle::UNDERLINE)),
                item("entity.name.class, entity.name.struct", Some(ac(10)), Some(FontStyle::UNDERLINE)),
                item("support.class.exception", Some(ac(14)), None),
                item("entity.name.function.decorator, meta.annotation", Some(ac(13)), Some(FontStyle::BOLD)),
                item("variable.other.constant, constant.other", Some(ac(9)), None),
                item("entity.other.attribute-name", Some(ac(14)), None),
                item("entity.name.tag", Some(ac(12)), None),
                item("string, punctuation.definition.string", Some(ac(3)), None),
                item("constant.numeric, constant.language", Some(ac(12)), None),
                item("markup.deleted", Some(ac(9)), None),
                item("markup.inserted", Some(ac(10)), None),
                item("markup.heading", None, Some(FontStyle::BOLD)),
                item("invalid", Some(ac(1)), Some(FontStyle::UNDERLINE)),
            ],
        }
    })
}

fn seg(s: syntect::highlighting::Style, text: &str) -> Seg {
    let c = s.foreground;
    let fg = match c.a {
        0 => Fg::Ansi(c.r),
        2 => Fg::Dim,
        _ => Fg::Default,
    };
    let f = s.font_style;
    Seg {
        text: text.trim_end_matches(['\n', '\r']).to_string(),
        fg,
        bold: f.contains(FontStyle::BOLD),
        italic: f.contains(FontStyle::ITALIC),
        underline: f.contains(FontStyle::UNDERLINE),
    }
}

/// Colore les `max_lines` premières lignes de `text` (syntaxe choisie d'après `name`).
/// Un texte avec des séquences d'échappement (sortie de terminal enregistrée) est rendu avec
/// ses propres couleurs : rien n'est envoyé tel quel au terminal.
pub fn highlight(name: &str, text: &str, max_lines: usize) -> Vec<Vec<Seg>> {
    if text.contains('\x1b') {
        return ansi(text, max_lines);
    }
    let ss = syntaxes();
    let first = text.lines().next().unwrap_or("");
    let syntax = ss
        .find_syntax_by_extension(name)
        .or_else(|| ss.find_syntax_by_extension(&ext_of(name)))
        .or_else(|| ss.find_syntax_by_first_line(first))
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut h = syntect::easy::HighlightLines::new(syntax, ansi_theme());
    let mut out = Vec::new();
    for line in syntect::util::LinesWithEndings::from(text).take(max_lines) {
        let line_exp = printable(&line.replace('\t', "    "));
        let segs = match h.highlight_line(&line_exp, ss) {
            Ok(parts) => parts.into_iter().map(|(s, t)| seg(s, t)).collect(),
            Err(_) => vec![Seg { text: line_exp.trim_end_matches(['\n', '\r']).to_string(), fg: Fg::Default, bold: false, italic: false, underline: false }],
        };
        out.push(segs);
    }
    out
}

/// Caractères de contrôle (hors fin de ligne) rendus visibles : `^X`, comme `cat -v`.
fn printable(line: &str) -> String {
    if !line.chars().any(|c| c.is_control() && c != '\n' && c != '\r') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len());
    for c in line.chars() {
        match c {
            '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 => {
                out.push('^');
                out.push((c as u8 + b'@') as char);
            }
            '\x7f' => out.push_str("^?"),
            c if c.is_control() => out.push('�'),
            c => out.push(c),
        }
    }
    out
}

/// Texte avec séquences ANSI : couleurs SGR (`ESC[…m`) appliquées, le reste (autres CSI,
/// OSC comme les liens `ESC]8;;…`) retiré.
fn ansi(text: &str, max_lines: usize) -> Vec<Vec<Seg>> {
    let mut out = Vec::new();
    let mut cur = Seg { text: String::new(), fg: Fg::Default, bold: false, italic: false, underline: false };
    for line in text.lines().take(max_lines) {
        let mut segs = Vec::new();
        let mut cs = line.chars().peekable();
        while let Some(c) = cs.next() {
            match c {
                '\x1b' => match cs.next() {
                    Some('[') => {
                        let mut params = String::new();
                        let mut fin = None;
                        for d in cs.by_ref() {
                            if ('\x40'..='\x7e').contains(&d) {
                                fin = Some(d);
                                break;
                            }
                            params.push(d);
                        }
                        if fin == Some('m') {
                            if !cur.text.is_empty() {
                                segs.push(cur.clone());
                                cur.text.clear();
                            }
                            sgr(&mut cur, &params);
                        }
                    }
                    // OSC : jusqu'à BEL ou ESC \\
                    Some(']') => {
                        while let Some(d) = cs.next() {
                            if d == '\x07' {
                                break;
                            }
                            if d == '\x1b' {
                                if cs.peek() == Some(&'\\') {
                                    cs.next();
                                }
                                break;
                            }
                        }
                    }
                    _ => {}
                },
                '\t' => cur.text.push_str("    "),
                '\r' => {}
                c => cur.text.push_str(&printable(&c.to_string())),
            }
        }
        if !cur.text.is_empty() {
            segs.push(cur.clone());
            cur.text.clear();
        }
        out.push(segs);
    }
    out
}

fn sgr(st: &mut Seg, params: &str) {
    let v: Vec<u16> = params.split(';').map(|p| p.parse().unwrap_or(0)).collect();
    let mut i = 0;
    while i < v.len() {
        match v[i] {
            0 => {
                st.fg = Fg::Default;
                st.bold = false;
                st.italic = false;
                st.underline = false;
            }
            1 => st.bold = true,
            2 => st.fg = Fg::Dim,
            3 => st.italic = true,
            4 => st.underline = true,
            22 => st.bold = false,
            23 => st.italic = false,
            24 => st.underline = false,
            n @ 30..=37 => st.fg = Fg::Ansi((n - 30) as u8),
            n @ 90..=97 => st.fg = Fg::Ansi((n - 90 + 8) as u8),
            39 => st.fg = Fg::Default,
            38 => {
                // 38;5;n (n < 16 seulement : la palette du terminal) ou 38;2;r;g;b
                match v.get(i + 1) {
                    Some(5) => {
                        st.fg = v.get(i + 2).filter(|&&n| n < 16).map(|&n| Fg::Ansi(n as u8)).unwrap_or(Fg::Default);
                        i += 2;
                    }
                    Some(2) => {
                        st.fg = Fg::Default;
                        i += 4;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        i += 1;
    }
}
