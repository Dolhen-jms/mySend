//! Aperçu des PDF : première page rendue en PNG par `pdftoppm`, nombre de pages lu par
//! `pdfinfo` (poppler). Un PDF distant est d'abord rapatrié dans le cache ; le rendu y reste.

use crate::config::{cache_dir, IMAGE_LIMIT};
use crate::fs::{Entry, Fs};
use crate::util::{ext_of, sha1_hex};
use std::io;
use std::path::PathBuf;
use std::time::Duration;

/// Largeur ou hauteur maximale du rendu, en pixels.
const SIZE: u32 = 1200;

pub struct PdfPage {
    /// Première page (PNG, dans le cache).
    pub png: PathBuf,
    pub pages: Option<u32>,
}

pub fn is_pdf(e: &Entry) -> bool {
    !e.is_dir && ext_of(&e.name) == "pdf" && e.size <= IMAGE_LIMIT
}

/// Rend la première page de `path` (appelé dans un thread).
pub fn first_page(fs: &Fs, path: &str, e: &Entry) -> io::Result<PdfPage> {
    let dir = cache_dir().join("pdf");
    std::fs::create_dir_all(&dir)?;
    let h = sha1_hex(format!("{}:{}:{}:{}", fs.target, path, e.mtime, e.size).as_bytes());
    let png = dir.join(format!("{h}.png"));
    let pages_file = dir.join(format!("{h}.pages"));
    if png.exists() {
        let pages = std::fs::read_to_string(&pages_file).ok().and_then(|s| s.trim().parse().ok());
        return Ok(PdfPage { png, pages });
    }
    let (src, fetched) = if fs.remote {
        let p = dir.join(format!("{h}.pdf"));
        let tmp = dir.join(format!("{h}.pdf.part"));
        fs.get(path, &tmp)?;
        std::fs::rename(&tmp, &p)?;
        (p, true)
    } else {
        (PathBuf::from(path), false)
    };
    let r = render(&src, &dir.join(&h));
    let pages = if r.is_ok() { count_pages(&src) } else { None };
    if fetched {
        let _ = std::fs::remove_file(&src);
    }
    r?;
    if let Some(n) = pages {
        let _ = std::fs::write(&pages_file, n.to_string());
    }
    Ok(PdfPage { png, pages })
}

/// `pdftoppm` écrit `<prefix>.png`.
fn render(src: &std::path::Path, prefix: &std::path::Path) -> io::Result<()> {
    let argv: Vec<String> = vec![
        "pdftoppm".into(),
        "-png".into(),
        "-f".into(),
        "1".into(),
        "-l".into(),
        "1".into(),
        "-singlefile".into(),
        "-scale-to".into(),
        SIZE.to_string(),
        src.to_string_lossy().into_owned(),
        prefix.to_string_lossy().into_owned(),
    ];
    let o = crate::proc::run(&argv, Some(Duration::from_secs(20)), None).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            io::Error::other("aperçu PDF : pdftoppm introuvable (paquet poppler-utils)")
        } else {
            e
        }
    })?;
    if !o.ok() {
        let msg = o.stderr.lines().next().unwrap_or("rendu impossible").trim().to_string();
        return Err(io::Error::other(format!("PDF : {msg}")));
    }
    Ok(())
}

fn count_pages(src: &std::path::Path) -> Option<u32> {
    let argv = vec!["pdfinfo".to_string(), src.to_string_lossy().into_owned()];
    let o = crate::proc::run(&argv, Some(Duration::from_secs(10)), None).ok()?;
    o.stdout.lines().find_map(|l| l.strip_prefix("Pages:")).and_then(|n| n.trim().parse().ok())
}
