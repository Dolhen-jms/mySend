//! Aperçu (colonne de droite) : dossier, image, texte coloré, binaire.

use crate::config::{cache_dir, IMAGE_LIMIT, PREVIEW_BYTES};
use crate::fs::{errstr, Entry, Fs};
use crate::style::{dim_italic, fg};
use crate::util::{ext_of, sha1_hex};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use crate::highlight::{Fg, Seg};

pub enum Preview {
    Dir { entries: Vec<Entry> },
    Image { dims: Option<(u32, u32)>, size: u64, image: Option<image::DynamicImage>, key: String },
    Special,
    Binary { entry: Entry },
    Text { lines: Vec<Line<'static>> },
    Error(String),
}

const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "ico"];

pub fn is_image(e: &Entry) -> bool {
    !e.is_dir && IMAGE_EXT.contains(&ext_of(&e.name).as_str()) && e.size <= IMAGE_LIMIT
}

/// Calcule l'aperçu de `path` (appelé dans un thread).
pub fn compute(fs: &Fs, path: &str, e: &Entry, height: usize) -> Preview {
    match compute_inner(fs, path, e, height) {
        Ok(p) => p,
        Err(err) => Preview::Error(errstr(&err)),
    }
}

fn compute_inner(fs: &Fs, path: &str, e: &Entry, height: usize) -> std::io::Result<Preview> {
    if e.is_dir {
        let entries = match fs.cached(path) {
            Some(v) => v,
            None => fs.ls(path)?,
        };
        return Ok(Preview::Dir { entries });
    }
    if crate::archive::archive_ext(&e.name).is_some() {
        return Ok(Preview::Text { lines: archive_lines(fs, path, height) });
    }
    if is_image(e) {
        let ext = ext_of(&e.name);
        let local = if fs.remote {
            let key = format!("{}:{}:{}:{}", fs.target, path, e.mtime, e.size);
            let dir = cache_dir().join("images");
            std::fs::create_dir_all(&dir)?;
            let p = dir.join(format!("{}.{ext}", sha1_hex(key.as_bytes())));
            if !p.exists() {
                let tmp = p.with_extension(format!("{ext}.part"));
                fs.get(path, &tmp)?;
                std::fs::rename(&tmp, &p)?;
            }
            p
        } else {
            std::path::PathBuf::from(path)
        };
        let dims = image::image_dimensions(&local).ok();
        let image = image::open(&local).ok();
        let key = format!("{}:{}:{}:{}", fs.target, path, e.mtime, e.size);
        return Ok(Preview::Image { dims, size: e.size, image, key });
    }
    let Some(data) = fs.read_head(path, PREVIEW_BYTES)? else {
        return Ok(Preview::Special);
    };
    if data.contains(&0) {
        return Ok(Preview::Binary { entry: e.clone() });
    }
    let text = String::from_utf8_lossy(&data).into_owned();
    Ok(Preview::Text { lines: highlight(&e.name, &text, height) })
}

// ---------------------------------------------------------------- coloration

fn conv(g: &Seg) -> Style {
    let mut st = Style::default();
    match g.fg {
        Fg::Ansi(n) => st = st.fg(ansi_idx(n)),
        Fg::Dim => st = st.add_modifier(Modifier::DIM),
        Fg::Default => {}
    }
    if g.bold {
        st = st.add_modifier(Modifier::BOLD);
    }
    if g.underline {
        st = st.add_modifier(Modifier::UNDERLINED);
    }
    if g.italic {
        st = st.add_modifier(Modifier::ITALIC);
    }
    st
}

fn ansi_idx(n: u8) -> Color {
    crate::style::sgr_style(&format!("38;5;{n}")).fg.unwrap_or(Color::Reset)
}

pub fn highlight(name: &str, text: &str, height: usize) -> Vec<Line<'static>> {
    crate::highlight::highlight(name, text, height)
        .into_iter()
        .map(|segs| Line::from(segs.iter().map(|g| Span::styled(g.text.clone(), conv(g))).collect::<Vec<_>>()))
        .collect()
}

pub fn special_line(msg: &str) -> Line<'static> {
    Line::from(Span::styled(msg.to_string(), dim_italic()))
}

pub fn error_line(msg: &str) -> Line<'static> {
    Line::from(Span::styled(msg.to_string(), fg(Color::Red)))
}

/// Contenu d'une archive : en-tête, puis les entrées (dossiers en bleu).
fn archive_lines(fs: &Fs, path: &str, height: usize) -> Vec<Line<'static>> {
    match crate::archive::list_contents(fs, path) {
        Err(e) => vec![error_line(&e)],
        Ok(v) => {
            let more = if v.len() >= 1000 { "+" } else { "" };
            let mut out = vec![special_line(&format!("Archive · {}{more} élément(s)", v.len())), Line::from("")];
            out.extend(v.into_iter().take(height.saturating_sub(2)).map(|n| {
                if n.ends_with('/') {
                    Line::from(Span::styled(n, crate::style::bold(crate::style::BLUE)))
                } else {
                    Line::from(n)
                }
            }));
            out
        }
    }
}
