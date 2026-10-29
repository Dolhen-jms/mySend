//! Volet d'aperçu : contenu d'un dossier, image, texte coloré, informations.

use crate::theme;
use crate::util::{bg, escape, icon_for, type_description};
use gtk::{pango, prelude::*};
use mysend::config::{cache_dir, IMAGE_LIMIT, PREVIEW_BYTES};
use mysend::fs::{errstr, Entry, FsRef};
use mysend::highlight::{highlight, Fg};
use mysend::util::{ext_of, filemode, fmt_time, human, natural_cmp, sha1_hex};
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "ico", "svg"];
const MAX_LINES: usize = 500;

enum Content {
    Dir(Vec<Entry>),
    /// Contenu d'une archive (chemins ; les dossiers finissent par /).
    Archive(Vec<String>),
    Image(PathBuf),
    Text(Vec<Vec<mysend::highlight::Seg>>),
    Binary,
    Special,
    Error(String),
}

pub struct Preview {
    pub root: gtk::Box,
    icon: gtk::Image,
    title: gtk::Label,
    meta: gtk::Label,
    stack: gtk::Stack,
    text: gtk::Label,
    picture: gtk::Picture,
    message: gtk::Label,
    generation: Cell<u64>,
}

impl Preview {
    pub fn new() -> Rc<Preview> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_width_request(360);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        head.add_css_class("preview-head");
        let icon = gtk::Image::new();
        icon.set_pixel_size(40);
        let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = gtk::Label::new(None);
        title.add_css_class("preview-title");
        title.set_xalign(0.0);
        title.set_ellipsize(pango::EllipsizeMode::Middle);
        title.set_selectable(true);
        let meta = gtk::Label::new(None);
        meta.add_css_class("muted");
        meta.set_xalign(0.0);
        meta.set_wrap(true);
        col.append(&title);
        col.append(&meta);
        head.append(&icon);
        head.append(&col);
        root.append(&head);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        let text = gtk::Label::new(None);
        text.add_css_class("preview-text");
        text.set_xalign(0.0);
        text.set_yalign(0.0);
        text.set_selectable(true);
        text.set_wrap(false);
        let sw = gtk::ScrolledWindow::builder().child(&text).build();
        stack.add_named(&sw, Some("text"));
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_margin_top(12);
        picture.set_margin_bottom(12);
        picture.set_margin_start(12);
        picture.set_margin_end(12);
        stack.add_named(&picture, Some("image"));
        let message = gtk::Label::new(None);
        message.add_css_class("muted");
        message.set_wrap(true);
        message.set_valign(gtk::Align::Center);
        stack.add_named(&message, Some("message"));
        root.append(&stack);
        Rc::new(Preview { root, icon, title, meta, stack, text, picture, message, generation: Cell::new(0) })
    }

    pub fn clear(&self, msg: &str) {
        self.generation.set(self.generation.get() + 1);
        self.icon.set_icon_name(None);
        self.title.set_text("");
        self.meta.set_text("");
        self.message.set_text(msg);
        self.stack.set_visible_child_name("message");
    }

    pub fn show(self: &Rc<Self>, fs: FsRef, path: String, e: Entry) {
        let g = self.generation.get() + 1;
        self.generation.set(g);
        self.icon.set_from_gicon(&icon_for(&e));
        self.title.set_text(&e.name);
        let mut meta = vec![type_description(&e)];
        if !e.is_dir {
            meta.push(human(e.size));
        }
        meta.push(fmt_time(e.mtime, "%d/%m/%Y %H:%M"));
        meta.push(filemode(e.mode));
        if e.is_link {
            meta.push("lien".into());
        }
        self.meta.set_text(&meta.join("  ·  "));
        let me = Rc::downgrade(self);
        bg(
            move || compute(&fs, &path, &e),
            move |c| {
                let Some(me) = me.upgrade() else { return };
                if me.generation.get() == g {
                    me.render(c);
                }
            },
        );
    }

    fn render(&self, c: Content) {
        let p = theme::current();
        match c {
            Content::Dir(mut v) => {
                if v.is_empty() {
                    self.message.set_text("Dossier vide");
                    self.stack.set_visible_child_name("message");
                    return;
                }
                v.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| natural_cmp(&a.name, &b.name)));
                let n = v.len();
                let mut out: Vec<String> = v
                    .iter()
                    .take(MAX_LINES)
                    .map(|e| {
                        if e.is_dir {
                            format!("<span foreground=\"{}\" weight=\"bold\">{}/</span>", p.blue, escape(&e.name))
                        } else {
                            escape(&e.name)
                        }
                    })
                    .collect();
                if n > MAX_LINES {
                    out.push(format!("<i>… et {} autres</i>", n - MAX_LINES));
                }
                self.text.set_markup(&out.join("\n"));
                self.stack.set_visible_child_name("text");
            }
            Content::Archive(v) => {
                let more = if v.len() >= 1000 { "+" } else { "" };
                let mut out = vec![format!("<i>Archive · {}{more} élément(s)</i>\n", v.len())];
                out.extend(v.iter().map(|n| {
                    if n.ends_with('/') {
                        format!("<span foreground=\"{}\" weight=\"bold\">{}</span>", p.blue, escape(n))
                    } else {
                        escape(n)
                    }
                }));
                self.text.set_markup(&out.join("\n"));
                self.stack.set_visible_child_name("text");
            }
            Content::Image(path) => {
                self.picture.set_filename(Some(&path));
                self.stack.set_visible_child_name("image");
            }
            Content::Text(lines) => {
                let mut out = String::new();
                for segs in lines {
                    for g in segs {
                        let t = escape(&g.text);
                        let color = match g.fg {
                            Fg::Ansi(n) => Some(p.ansi[(n as usize).min(15)].as_str()),
                            Fg::Dim => Some(p.muted.as_str()),
                            Fg::Default => None,
                        };
                        let mut attrs = String::new();
                        if let Some(c) = color {
                            attrs.push_str(&format!(" foreground=\"{c}\""));
                        }
                        if g.bold {
                            attrs.push_str(" weight=\"bold\"");
                        }
                        if g.italic {
                            attrs.push_str(" style=\"italic\"");
                        }
                        if g.underline {
                            attrs.push_str(" underline=\"single\"");
                        }
                        if attrs.is_empty() {
                            out.push_str(&t);
                        } else {
                            out.push_str(&format!("<span{attrs}>{t}</span>"));
                        }
                    }
                    out.push('\n');
                }
                self.text.set_markup(out.trim_end_matches('\n'));
                self.stack.set_visible_child_name("text");
            }
            Content::Binary => {
                self.message.set_text("Fichier binaire");
                self.stack.set_visible_child_name("message");
            }
            Content::Special => {
                self.message.set_text("Fichier spécial");
                self.stack.set_visible_child_name("message");
            }
            Content::Error(e) => {
                self.message.set_text(&e);
                self.stack.set_visible_child_name("message");
            }
        }
    }
}

fn compute(fs: &FsRef, path: &str, e: &Entry) -> Content {
    let r = (|| -> std::io::Result<Content> {
        if e.is_dir {
            return Ok(Content::Dir(match fs.cached(path) {
                Some(v) => v,
                None => fs.ls(path)?,
            }));
        }
        if mysend::archive::archive_ext(&e.name).is_some() {
            return Ok(Content::Archive(mysend::archive::list_contents(fs, path).map_err(std::io::Error::other)?));
        }
        if mysend::pdf::is_pdf(e) {
            return Ok(Content::Image(mysend::pdf::first_page(fs, path, e)?.png));
        }
        let ext = ext_of(&e.name);
        if IMAGE_EXT.contains(&ext.as_str()) && e.size <= IMAGE_LIMIT {
            if !fs.remote {
                return Ok(Content::Image(PathBuf::from(path)));
            }
            let key = format!("{}:{}:{}:{}", fs.target, path, e.mtime, e.size);
            let dir = cache_dir().join("images");
            std::fs::create_dir_all(&dir)?;
            let p = dir.join(format!("{}.{ext}", sha1_hex(key.as_bytes())));
            if !p.exists() {
                let tmp = p.with_extension(format!("{ext}.part"));
                fs.get(path, &tmp)?;
                std::fs::rename(&tmp, &p)?;
            }
            return Ok(Content::Image(p));
        }
        let Some(data) = fs.read_head(path, PREVIEW_BYTES)? else { return Ok(Content::Special) };
        if data.contains(&0) {
            return Ok(Content::Binary);
        }
        let text = String::from_utf8_lossy(&data);
        Ok(Content::Text(highlight(&e.name, &text, MAX_LINES)))
    })();
    r.unwrap_or_else(|err| Content::Error(errstr(&err)))
}

