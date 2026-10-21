//! Thème du bureau (`~/.local/state/theme/current`) : palette de `wayle.scss` (`--palette-*`)
//! et 16 couleurs ANSI de `kitty.conf`. Commun aux deux interfaces.
//! Facultatif : sans ce dossier (autre machine, macOS), la TUI garde les couleurs du terminal
//! et la GUI celles du système.

use regex::Regex;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct Palette {
    pub bg: String,
    pub surface: String,
    pub elevated: String,
    pub fg: String,
    pub muted: String,
    pub primary: String,
    pub red: String,
    pub yellow: String,
    pub green: String,
    pub blue: String,
    pub ansi: [String; 16],
}

impl Default for Palette {
    /// Catppuccin Macchiato : le thème « actuel ».
    fn default() -> Self {
        let s = |x: &str| x.to_string();
        let ansi = [
            "#494d64", "#ed8796", "#a6da95", "#eed49f", "#8aadf4", "#f5bde6", "#8bd5ca", "#b8c0e0", "#5b6078", "#ed8796",
            "#a6da95", "#eed49f", "#8aadf4", "#f5bde6", "#8bd5ca", "#a5adcb",
        ]
        .map(s);
        Palette {
            bg: s("#24273a"),
            surface: s("#1e2030"),
            elevated: s("#363a4f"),
            fg: s("#cad3f5"),
            muted: s("#8087a2"),
            primary: s("#8aadf4"),
            red: s("#ed8796"),
            yellow: s("#eed49f"),
            green: s("#a6da95"),
            blue: s("#8aadf4"),
            ansi,
        }
    }
}

pub fn state_dir() -> PathBuf {
    PathBuf::from(crate::config::home_dir()).join(".local/state/theme")
}

pub fn rgb_f(hex: &str) -> (f64, f64, f64) {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    (((v >> 16) & 255) as f64 / 255.0, ((v >> 8) & 255) as f64 / 255.0, (v & 255) as f64 / 255.0)
}

impl Palette {
    pub fn load() -> Palette {
        let mut p = Palette::default();
        let dir = state_dir().join("current");
        let hex = r"(#[0-9a-fA-F]{6})\b";
        if let Ok(t) = std::fs::read_to_string(dir.join("wayle.scss")) {
            let re = Regex::new(&format!(r"--palette-([\w-]+)\s*:\s*{hex}")).unwrap();
            for c in re.captures_iter(&t) {
                let v = c[2].to_string();
                match &c[1] {
                    "bg" => p.bg = v,
                    "surface" => p.surface = v,
                    "elevated" => p.elevated = v,
                    "fg" => p.fg = v,
                    "fg-muted" => p.muted = v,
                    "primary" => p.primary = v,
                    "red" => p.red = v,
                    "yellow" => p.yellow = v,
                    "green" => p.green = v,
                    "blue" => p.blue = v,
                    _ => {}
                }
            }
            // sans kitty.conf : ANSI dérivées de la palette
            let fill = [&p.elevated, &p.red, &p.green, &p.yellow, &p.blue, &p.primary, &p.blue, &p.fg];
            for i in 0..16 {
                p.ansi[i] = fill[i % 8].clone();
            }
            p.ansi[8] = p.muted.clone();
        }
        if let Ok(t) = std::fs::read_to_string(dir.join("kitty.conf")) {
            let re = Regex::new(&format!(r"(?m)^\s*color(\d+)\s+{hex}")).unwrap();
            for c in re.captures_iter(&t) {
                if let Ok(i) = c[1].parse::<usize>() {
                    if i < 16 {
                        p.ansi[i] = c[2].to_string();
                    }
                }
            }
        }
        p
    }

    pub fn is_dark(&self) -> bool {
        let (r, g, b) = rgb_f(&self.bg);
        0.2126 * r + 0.7152 * g + 0.0722 * b < 0.5
    }

}

/// Couleur `#rrggbb` en octets.
pub fn rgb(hex: &str) -> (u8, u8, u8) {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    ((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Empreinte du thème courant : change quand on bascule (`theme-set`) ou qu'on retouche ses fichiers.
pub fn stamp() -> Option<(PathBuf, Option<SystemTime>, Option<SystemTime>)> {
    let cur = state_dir().join("current");
    let target = std::fs::read_link(&cur).ok()?;
    let m = |f: &str| std::fs::metadata(cur.join(f)).and_then(|m| m.modified()).ok();
    Some((target, m("wayle.scss"), m("kitty.conf")))
}
