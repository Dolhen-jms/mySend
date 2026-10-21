//! Couleurs (palette ANSI du terminal), LS_COLORS, icônes Nerd Font, droits.

use crate::config::icons;
use crate::fs::Entry;
use crate::util::ext_of;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use std::collections::HashMap;
use std::sync::OnceLock;

pub const BLACK: Color = Color::Black;
pub const RED: Color = Color::Red;
pub const GREEN: Color = Color::Green;
pub const YELLOW: Color = Color::Yellow;
pub const BLUE: Color = Color::Blue;
pub const MAGENTA: Color = Color::Magenta;
pub const CYAN: Color = Color::Cyan;
pub const WHITE: Color = Color::Gray; // ANSI 7
pub const BRIGHT_BLACK: Color = Color::DarkGray; // ANSI 8

// ---------------------------------------------------------------- thème du bureau

/// Couleurs du thème (si le terminal affiche les couleurs 24 bits) : mêmes rôles que l'interface GTK.
struct Pal {
    accent: Color,
    surface: Color,
    cursor: Color,
    parent_cursor: Color,
    ansi: [Color; 16],
}

static PAL: std::sync::RwLock<Option<Pal>> = std::sync::RwLock::new(None);

fn truecolor() -> bool {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    matches!(env("COLORTERM").as_str(), "truecolor" | "24bit")
        || !env("KITTY_WINDOW_ID").is_empty()
        // terminaux macOS qui ne posent pas COLORTERM partout
        || matches!(env("TERM_PROGRAM").as_str(), "iTerm.app" | "WezTerm" | "ghostty")
        || ["kitty", "ghostty", "wezterm", "alacritty", "foot"].iter().any(|t| env("TERM").contains(t))
}

fn rgb(hex: &str) -> Color {
    let (r, g, b) = crate::theme::rgb(hex);
    Color::Rgb(r, g, b)
}

/// `a` posé sur `b` avec l'opacité `t`.
fn mix(a: &str, b: &str, t: f32) -> Color {
    let (a, b) = (crate::theme::rgb(a), crate::theme::rgb(b));
    let m = |x: u8, y: u8| (x as f32 * t + y as f32 * (1.0 - t)).round() as u8;
    Color::Rgb(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// (Re)lit le thème courant ; sans thème ou sans couleurs 24 bits : palette du terminal.
pub fn load_palette() {
    let p = (truecolor() && crate::theme::stamp().is_some()).then(|| {
        let t = crate::theme::Palette::load();
        Pal {
            accent: rgb(&t.primary),
            surface: rgb(&t.surface),
            cursor: mix(&t.primary, &t.bg, 0.28),
            parent_cursor: rgb(&t.elevated),
            ansi: std::array::from_fn(|i| rgb(&t.ansi[i])),
        }
    });
    *PAL.write().unwrap() = p;
}

/// Accent : cadres, sélection.
pub fn accent() -> Color {
    PAL.read().unwrap().as_ref().map(|p| p.accent).unwrap_or(YELLOW)
}

/// Fond des fenêtres (dialogues, notifications).
pub fn surface() -> Color {
    PAL.read().unwrap().as_ref().map(|p| p.surface).unwrap_or(BLACK)
}

/// Ligne sous le curseur : teinte d'accent (vidéo inverse sans thème).
pub fn cursor_style() -> Style {
    match PAL.read().unwrap().as_ref() {
        Some(p) => Style::default().bg(p.cursor),
        None => Style::default().add_modifier(Modifier::REVERSED),
    }
}

/// Curseur de la colonne parente.
pub fn parent_cursor_bg() -> Color {
    PAL.read().unwrap().as_ref().map(|p| p.parent_cursor).unwrap_or(BRIGHT_BLACK)
}

/// Remplace les 16 couleurs ANSI de l'écran par celles du thème (même rendu dans tout terminal).
pub fn remap(buf: &mut ratatui::buffer::Buffer) {
    let pal = PAL.read().unwrap();
    let Some(p) = pal.as_ref() else { return };
    let map = |c: Color| match c {
        Color::Black => p.ansi[0],
        Color::Red => p.ansi[1],
        Color::Green => p.ansi[2],
        Color::Yellow => p.ansi[3],
        Color::Blue => p.ansi[4],
        Color::Magenta => p.ansi[5],
        Color::Cyan => p.ansi[6],
        Color::Gray => p.ansi[7],
        Color::DarkGray => p.ansi[8],
        Color::LightRed => p.ansi[9],
        Color::LightGreen => p.ansi[10],
        Color::LightYellow => p.ansi[11],
        Color::LightBlue => p.ansi[12],
        Color::LightMagenta => p.ansi[13],
        Color::LightCyan => p.ansi[14],
        Color::White => p.ansi[15],
        Color::Indexed(n) if n < 16 => p.ansi[n as usize],
        c => c,
    };
    for cell in buf.content.iter_mut() {
        cell.fg = map(cell.fg);
        cell.bg = map(cell.bg);
    }
}

pub fn fg(c: Color) -> Style {
    Style::default().fg(c)
}
pub fn bold(c: Color) -> Style {
    Style::default().fg(c).add_modifier(Modifier::BOLD)
}
pub fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}
pub fn dim_italic() -> Style {
    Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC)
}
pub fn b() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

fn ansi(n: u8) -> Color {
    match n {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::Gray,
        8 => Color::DarkGray,
        9 => Color::LightRed,
        10 => Color::LightGreen,
        11 => Color::LightYellow,
        12 => Color::LightBlue,
        13 => Color::LightMagenta,
        14 => Color::LightCyan,
        15 => Color::White,
        n => Color::Indexed(n),
    }
}

/// Code SGR (`01;34`) → style.
pub fn sgr_style(code: &str) -> Style {
    let nums: Vec<u32> = code.split(';').map(|s| s.parse().unwrap_or(0)).collect();
    let mut st = Style::default();
    let mut i = 0;
    while i < nums.len() {
        let n = nums[i];
        match n {
            0 => st = Style::default(),
            1 => st = st.add_modifier(Modifier::BOLD),
            2 => st = st.add_modifier(Modifier::DIM),
            3 => st = st.add_modifier(Modifier::ITALIC),
            4 => st = st.add_modifier(Modifier::UNDERLINED),
            5 => st = st.add_modifier(Modifier::SLOW_BLINK),
            7 => st = st.add_modifier(Modifier::REVERSED),
            9 => st = st.add_modifier(Modifier::CROSSED_OUT),
            30..=37 => st = st.fg(ansi((n - 30) as u8)),
            40..=47 => st = st.bg(ansi((n - 40) as u8)),
            90..=97 => st = st.fg(ansi((n - 90 + 8) as u8)),
            100..=107 => st = st.bg(ansi((n - 100 + 8) as u8)),
            39 => st = st.fg(Color::Reset),
            49 => st = st.bg(Color::Reset),
            38 | 48 => {
                let col = match nums.get(i + 1) {
                    Some(5) => {
                        let c = nums.get(i + 2).copied().unwrap_or(0) as u8;
                        i += 2;
                        Some(ansi(c))
                    }
                    Some(2) => {
                        let g = |k: usize| nums.get(i + k).copied().unwrap_or(0) as u8;
                        let c = Color::Rgb(g(2), g(3), g(4));
                        i += 4;
                        Some(c)
                    }
                    _ => None,
                };
                if let Some(c) = col {
                    st = if n == 38 { st.fg(c) } else { st.bg(c) };
                }
            }
            _ => {}
        }
        i += 1;
    }
    st
}

struct LsColors {
    by_type: HashMap<String, Style>,
    by_name: HashMap<String, Style>,
}

fn ls_colors() -> &'static LsColors {
    static LS: OnceLock<LsColors> = OnceLock::new();
    LS.get_or_init(|| {
        let mut by_type = HashMap::new();
        let mut by_name = HashMap::new();
        let raw = std::env::var("LS_COLORS").unwrap_or_default();
        for item in raw.split(':') {
            let Some((k, v)) = item.split_once('=') else { continue };
            let st = sgr_style(v);
            if let Some(rest) = k.strip_prefix("*.") {
                by_name.insert(rest.to_lowercase(), st);
            } else if let Some(rest) = k.strip_prefix('*') {
                by_name.insert(rest.to_lowercase(), st);
            } else {
                by_type.insert(k.to_string(), st);
            }
        }
        LsColors { by_type, by_name }
    })
}

pub fn name_style(e: &Entry) -> Style {
    let ls = ls_colors();
    let t = |k: &str, d: Style| ls.by_type.get(k).copied().unwrap_or(d);
    let fmt = e.mode & 0o170000;
    if e.is_dir {
        return t("di", bold(BLUE));
    }
    if e.is_link {
        return t("ln", fg(CYAN));
    }
    if fmt == 0o010000 {
        return t("pi", fg(YELLOW));
    }
    if fmt == 0o140000 {
        return t("so", fg(MAGENTA));
    }
    if e.mode & 0o111 != 0 {
        return t("ex", bold(GREEN));
    }
    let lname = e.name.to_lowercase();
    if let Some(s) = ls.by_name.get(&lname) {
        return *s;
    }
    let ext = ext_of(&e.name);
    if !ext.is_empty() {
        if let Some(s) = ls.by_name.get(&ext) {
            return *s;
        }
    }
    ls.by_type.get("fi").copied().unwrap_or_default()
}

pub fn icon(e: &Entry) -> (char, Color) {
    let lname = e.name.to_lowercase();
    let by_name = match lname.as_str() {
        ".git" => Some(('\u{e5fb}', RED)),
        ".config" => Some(('\u{e5fc}', BLUE)),
        "downloads" | "téléchargements" => Some(('\u{f019}', CYAN)),
        "pictures" | "images" => Some(('\u{f03e}', MAGENTA)),
        "music" | "musique" => Some(('\u{f001}', MAGENTA)),
        "videos" | "vidéos" => Some(('\u{f03d}', MAGENTA)),
        "dockerfile" => Some(('\u{f308}', BLUE)),
        "flake.nix" => Some(('\u{f313}', BLUE)),
        "flake.lock" => Some(('\u{f023}', BRIGHT_BLACK)),
        "makefile" => Some(('\u{f489}', YELLOW)),
        "license" => Some(('\u{f15c}', YELLOW)),
        ".gitignore" => Some(('\u{e702}', RED)),
        "todo.md" => Some(('\u{f0ae}', YELLOW)),
        _ => None,
    };
    if let Some(i) = by_name {
        return i;
    }
    if e.is_dir {
        return ('\u{f07b}', BLUE);
    }
    match ext_of(&e.name).as_str() {
        "py" | "pyi" => ('\u{e73c}', YELLOW),
        "rs" => ('\u{e7a8}', RED),
        "go" => ('\u{e626}', CYAN),
        "nix" => ('\u{f313}', BLUE),
        "js" | "mjs" | "cjs" => ('\u{e74e}', YELLOW),
        "ts" | "tsx" => ('\u{e628}', BLUE),
        "html" | "htm" => ('\u{e736}', RED),
        "css" | "scss" => ('\u{e749}', BLUE),
        "md" | "markdown" => ('\u{e73e}', WHITE),
        "json" => ('\u{e60b}', YELLOW),
        "lua" => ('\u{e620}', BLUE),
        "java" => ('\u{e738}', RED),
        "c" | "h" => ('\u{e61e}', BLUE),
        "cpp" | "cc" | "hpp" => ('\u{e61d}', BLUE),
        "sh" | "bash" | "zsh" | "fish" => ('\u{f489}', GREEN),
        "toml" | "yaml" | "yml" | "ini" | "conf" | "cfg" => ('\u{e615}', BRIGHT_BLACK),
        "qml" => ('\u{f375}', GREEN),
        "lock" => ('\u{f023}', BRIGHT_BLACK),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico" | "tiff" => ('\u{f1c5}', MAGENTA),
        "mp4" | "mkv" | "webm" | "avi" | "mov" => ('\u{f03d}', MAGENTA),
        "mp3" | "flac" | "ogg" | "wav" | "opus" | "m4a" => ('\u{f001}', MAGENTA),
        "pdf" => ('\u{f1c1}', RED),
        "zip" | "tar" | "gz" | "tgz" | "xz" | "bz2" | "zst" | "7z" | "rar" => ('\u{f410}', YELLOW),
        "txt" | "log" => ('\u{f15c}', WHITE),
        "pem" | "key" | "pub" | "gpg" | "asc" => ('\u{f084}', YELLOW),
        _ => ('\u{f15b}', WHITE),
    }
}

/// Droits colorés caractère par caractère.
pub fn perms(mode: u32) -> Vec<Span<'static>> {
    crate::util::filemode(mode)
        .chars()
        .map(|c| {
            let col = match c {
                'd' => BLUE,
                'l' => CYAN,
                'r' => YELLOW,
                'w' => RED,
                'x' => GREEN,
                's' | 't' | 'S' | 'T' => MAGENTA,
                '-' => BRIGHT_BLACK,
                _ => Color::Reset,
            };
            Span::styled(c.to_string(), fg(col))
        })
        .collect()
}

pub fn use_icons() -> bool {
    icons()
}
