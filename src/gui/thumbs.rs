//! Miniatures des images dans la grille : décodées dans un thread, gardées en mémoire et sur
//! disque (~/.cache/mysend/thumbs) ; une image distante n'est téléchargée qu'une fois.

use crate::util::bg;
use gtk::{gdk, glib, prelude::*};
use mysend::config::{cache_dir, IMAGE_LIMIT};
use mysend::fs::{Entry, FsRef};
use mysend::util::{ext_of, sha1_hex};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

/// Côté de la miniature, en pixels.
const SIZE: u32 = 160;
const EXT: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif", "ico"];

thread_local! {
    static MEM: RefCell<HashMap<String, gdk::Texture>> = RefCell::new(HashMap::new());
}

pub fn is_image(e: &Entry) -> bool {
    !e.is_dir && e.size > 0 && e.size <= IMAGE_LIMIT && EXT.contains(&ext_of(&e.name).as_str())
}

/// Pixels RGBA d'une miniature.
struct Thumb {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

fn make(fs: &FsRef, path: &str, key: &str, ext: &str) -> Option<Thumb> {
    let dir = cache_dir().join("thumbs");
    std::fs::create_dir_all(&dir).ok()?;
    let thumb_file = dir.join(format!("{}.png", sha1_hex(key.as_bytes())));
    let img = match image::open(&thumb_file) {
        Ok(i) => i,
        Err(_) => {
            // l'image elle-même : sur place en local, téléchargée à côté du cache à distance
            let (src, tmp): (PathBuf, Option<PathBuf>) = if fs.remote || fs.sudo.is_some() {
                let t = dir.join(format!("{}.part.{ext}", sha1_hex(key.as_bytes())));
                fs.get(path, &t).ok()?;
                (t.clone(), Some(t))
            } else {
                (PathBuf::from(path), None)
            };
            let r = image::open(&src).map(|i| i.thumbnail(SIZE, SIZE));
            if let Some(t) = tmp {
                let _ = std::fs::remove_file(t);
            }
            let t = r.ok()?;
            let _ = t.save_with_format(&thumb_file, image::ImageFormat::Png);
            t
        }
    };
    let rgba = img.to_rgba8();
    Some(Thumb { w: rgba.width(), h: rgba.height(), rgba: rgba.into_raw() })
}

/// Met la miniature de `path` dans `img` dès qu'elle est prête (si la case montre toujours
/// cette image : les cases de la grille sont recyclées pendant le défilement).
pub fn load(img: &gtk::Image, fs: FsRef, path: String, e: &Entry) {
    let key = format!("{}:{}:{}:{}", fs.target, path, e.mtime, e.size);
    img.set_widget_name(&key);
    if let Some(t) = MEM.with(|m| m.borrow().get(&key).cloned()) {
        img.set_paintable(Some(&t));
        return;
    }
    let ext = ext_of(&e.name);
    let weak = img.downgrade();
    let k2 = key.clone();
    bg(
        move || make(&fs, &path, &k2, &ext),
        move |r| {
            let Some(t) = r else { return };
            let bytes = glib::Bytes::from_owned(t.rgba);
            let tex: gdk::Texture = gdk::MemoryTexture::new(t.w as i32, t.h as i32, gdk::MemoryFormat::R8g8b8a8, &bytes, t.w as usize * 4).upcast();
            MEM.with(|m| {
                let mut m = m.borrow_mut();
                if m.len() > 3000 {
                    m.clear();
                }
                m.insert(key.clone(), tex.clone());
            });
            if let Some(img) = weak.upgrade() {
                if img.widget_name() == key {
                    img.set_paintable(Some(&tex));
                }
            }
        },
    );
}
