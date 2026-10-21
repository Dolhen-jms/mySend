//! Corbeille freedesktop (locale et distante).

use crate::fs::{errstr, Fs};
use crate::util::{basename, dirname, join, url_quote, url_unquote};
use std::io;

#[derive(Debug, Clone)]
pub struct TrashItem {
    pub name: String,
    pub path: String,
    pub date: String,
}

fn root(fs: &Fs) -> String {
    join(&fs.home, ".local/share/Trash")
}

fn dirs(fs: &Fs) -> io::Result<(String, String)> {
    let r = root(fs);
    let (f, i) = (join(&r, "files"), join(&r, "info"));
    fs.makedirs(&f)?;
    fs.makedirs(&i)?;
    Ok((f, i))
}

pub fn put(fs: &Fs, path: &str) -> io::Result<()> {
    let (files, info) = dirs(fs)?;
    let base = basename(path).to_string();
    let mut name = base.clone();
    let mut n = 1;
    while fs.exists(&join(&files, &name)) || fs.exists(&join(&info, &format!("{name}.trashinfo"))) {
        n += 1;
        name = format!("{base}.{n}");
    }
    let info_path = join(&info, &format!("{name}.trashinfo"));
    let text = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        url_quote(path),
        chrono::Local::now().format("%Y-%m-%dT%H:%M:%S")
    );
    fs.write_bytes(&info_path, text.as_bytes())?;
    if let Err(e) = fs.rename(path, &join(&files, &name)) {
        let _ = fs.remove(&info_path);
        return Err(io::Error::other(format!(
            "{} — la corbeille est peut-être sur un autre disque (D supprime définitivement)",
            errstr(&e)
        )));
    }
    Ok(())
}

pub fn list(fs: &Fs) -> io::Result<Vec<TrashItem>> {
    let info = join(&root(fs), "info");
    let entries = match fs.listdir(&info) {
        Ok(v) => v,
        Err(_) => return Ok(Vec::new()),
    };
    let mut out = Vec::new();
    for e in entries {
        let Some(name) = e.name.strip_suffix(".trashinfo") else { continue };
        let Ok(Some(data)) = fs.read_head(&join(&info, &e.name), 8192) else { continue };
        let text = String::from_utf8_lossy(&data);
        let mut path = String::new();
        let mut date = String::new();
        for l in text.lines() {
            if let Some(v) = l.strip_prefix("Path=") {
                path = url_unquote(v.trim());
            } else if let Some(v) = l.strip_prefix("DeletionDate=") {
                date = v.trim().replace('T', " ");
            }
        }
        out.push(TrashItem { name: name.to_string(), path, date });
    }
    out.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(out)
}

pub fn restore(fs: &Fs, item: &TrashItem) -> io::Result<String> {
    let r = root(fs);
    let src = join(&r, &format!("files/{}", item.name));
    let mut dst = item.path.clone();
    if fs.exists(&dst) {
        let is_dir = fs.stat(&src).map(|e| e.is_dir).unwrap_or(false);
        dst = fs.unique(&dst, is_dir);
    }
    fs.makedirs(&dirname(&dst))?;
    fs.move_(&src, &dst)?;
    let _ = fs.remove(&join(&r, &format!("info/{}.trashinfo", item.name)));
    Ok(dst)
}

pub fn delete(fs: &Fs, item: &TrashItem) -> io::Result<()> {
    let r = root(fs);
    let f = join(&r, &format!("files/{}", item.name));
    if fs.exists(&f) {
        fs.remove(&f)?;
    }
    fs.remove(&join(&r, &format!("info/{}.trashinfo", item.name)))
}

pub fn empty(fs: &Fs) -> io::Result<()> {
    let r = root(fs);
    for d in ["files", "info"] {
        let p = join(&r, d);
        if fs.exists(&p) {
            fs.remove(&p)?;
        }
        fs.makedirs(&p)?;
    }
    Ok(())
}
