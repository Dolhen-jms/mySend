//! Clavier de la liste de fichiers (raccourcis classiques d'un gestionnaire de fichiers),
//! et commandes des menus : compresser, extraire, chercher dans les sous-dossiers.

use crate::dialogs::{self, Field};
use crate::pane::Pane;
use crate::window::{win, Win};
use gtk::{gdk, prelude::*};
use mysend::util::join;
use std::rc::Rc;

impl Win {
    /// Touches de la liste de fichiers. Vrai si la touche est prise.
    pub fn list_key(self: &Rc<Self>, p: &Rc<Pane>, key: gdk::Key, state: gdk::ModifierType) -> bool {
        use gdk::Key;
        let m = state & (gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::ALT_MASK);
        let ctrl = gdk::ModifierType::CONTROL_MASK;
        let shift = gdk::ModifierType::SHIFT_MASK;
        let alt = gdk::ModifierType::ALT_MASK;
        match (key.to_lower(), m) {
            (Key::z, x) if x == ctrl => self.undo_last(),
            (Key::c, x) if x == ctrl => self.yank(false),
            (Key::x, x) if x == ctrl => self.yank(true),
            (Key::v, x) if x == ctrl => self.paste(false),
            (Key::v, x) if x == ctrl | shift => self.paste(true),
            (Key::c, x) if x == ctrl | shift => self.copy_path(),
            (Key::n, x) if x == ctrl | shift => self.create(true),
            (Key::n, x) if x == ctrl => self.create(false),
            (Key::d, x) if x == ctrl => self.add_bookmark(),
            (Key::b, x) if x == ctrl => self.bookmarks_dialog(),
            (Key::f, x) if x == ctrl | shift => self.recursive_search(p, false),
            (Key::Delete | Key::KP_Delete, x) if x.is_empty() => self.delete(false),
            (Key::Delete | Key::KP_Delete, x) if x == shift => self.delete(true),
            (Key::F2, x) if x.is_empty() => self.rename(),
            (Key::Menu, x) if x.is_empty() => p.popup_menu_here(),
            (Key::F10, x) if x == shift => p.popup_menu_here(),
            (Key::BackSpace, x) if x.is_empty() => p.go_up(),
            (Key::Tab, x) if x.is_empty() && self.dual.get() => {
                if let Some(o) = self.other_pane() {
                    o.focus_list();
                }
            }
            (Key::Up, x) if x == alt => p.go_up(),
            (Key::Left, x) if x == alt => p.go_back(),
            (Key::Right, x) if x == alt => p.go_forward(),
            (Key::Escape, x) if x.is_empty() => {
                if p.search_bar.is_search_mode() {
                    return false;
                }
                p.selection.unselect_all();
            }
            _ => return false,
        }
        true
    }

    pub(crate) fn compress(self: &Rc<Self>, p: &Rc<Pane>) {
        let items = p.selected();
        if items.is_empty() {
            return;
        }
        let (fs, cwd) = (p.fs.borrow().clone(), p.cwd.borrow().clone());
        let base = if items.len() == 1 {
            items[0].1.name.clone()
        } else {
            let b = mysend::util::basename(&cwd);
            if b.is_empty() { "archive".into() } else { b.to_string() }
        };
        let default = format!("{base}.tar.gz");
        let stem = base.chars().count() as i32;
        let pw = Rc::downgrade(p);
        dialogs::form(
            &self.window,
            "Compresser",
            "Le format suit l'extension : .tar.gz, .tar.xz, .tar.zst, .zip…",
            &[Field::Text("Nom de l'archive", &default)],
            "Compresser",
            Some((0, stem)),
            move |v| {
                let name = v[0].trim().to_string();
                if name.is_empty() {
                    return;
                }
                let out = mysend::util::normpath(&join(&cwd, &name));
                let task = mysend::archive::compress_task(fs, cwd, &items, out);
                win().enqueue(task);
                let _ = pw;
            },
        );
    }

    pub(crate) fn extract(self: &Rc<Self>, p: &Rc<Pane>) {
        let Some((_, e)) = p.current() else { return };
        if e.is_dir || mysend::archive::archive_ext(&e.name).is_none() {
            self.toast("Pas une archive reconnue (tar, zip, 7z, gz, xz, bz2, zst).");
            return;
        }
        let task = mysend::archive::extract_task(p.fs.borrow().clone(), p.cwd.borrow().clone(), &e.name);
        self.enqueue(task);
    }

    /// s / S : recherche dans l'arborescence (par nom, dans le contenu), résultats dans une liste.
    pub(crate) fn recursive_search(self: &Rc<Self>, p: &Rc<Pane>, content: bool) {
        let (fs, cwd) = (p.fs.borrow().clone(), p.cwd.borrow().clone());
        let heading = if content { "Chercher dans le contenu" } else { "Chercher par nom" };
        let pw = Rc::downgrade(p);
        dialogs::form(&self.window, heading, "Dans ce dossier et ses sous-dossiers.", &[Field::Text("Texte", "")], "Chercher", None, move |v| {
            let pat = v[0].clone();
            if pat.is_empty() {
                return;
            }
            win().toast(&format!("Recherche de « {pat} »…"));
            let (fs2, cwd2, pat2) = (fs.clone(), cwd.clone(), pat.clone());
            crate::util::bg(
                move || mysend::archive::find(&fs2, &cwd2, &pat2, content),
                move |r| {
                    let w = win();
                    let (items, err) = match r {
                        Ok(v) => v,
                        Err(e) => (Vec::new(), e),
                    };
                    if items.is_empty() {
                        let mut msg = format!("Rien trouvé pour « {pat} ».");
                        if !err.trim().is_empty() {
                            msg.push(' ');
                            msg.push_str(err.trim());
                        }
                        w.toast(&msg);
                        return;
                    }
                    if let Some(p) = pw.upgrade() {
                        let title = format!("{}{} résultat(s) pour « {pat} »", items.len(), if items.len() >= 2000 { "+" } else { "" });
                        crate::extras::results_dialog(&w, &p, &title, cwd, items);
                    }
                },
            );
        });
    }
}
