//! Fenêtres des touches ' (marque-pages), T (corbeille), s / S (résultats de recherche).

use crate::pane::Pane;
use crate::util::bg;
use crate::window::{win, Win};
use adw::prelude::*;
use gtk::{gdk, glib};
use mysend::fs::{errstr, FsRef};
use mysend::trash::{self, TrashItem};
use mysend::util::{basename, dirname, join};
use std::rc::Rc;

/// Fenêtre à liste : titre, contenu, aide en bas.
fn list_dialog(title: &str, width: i32, hint: &str) -> (adw::Dialog, gtk::ListBox, gtk::Box) {
    let d = adw::Dialog::new();
    d.set_title(title);
    d.set_content_width(width);
    d.set_content_height(520);
    let tb = adw::ToolbarView::new();
    let hb = adw::HeaderBar::new();
    tb.add_top_bar(&hb);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 10);
    col.set_margin_top(6);
    col.set_margin_bottom(14);
    col.set_margin_start(14);
    col.set_margin_end(14);
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    let sw = gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
    col.append(&sw);
    let h = gtk::Label::new(Some(hint));
    h.add_css_class("muted");
    col.append(&h);
    tb.set_content(Some(&col));
    d.set_child(Some(&tb));
    (d, list, col)
}

fn esc(s: &str) -> String {
    glib::markup_escape_text(s).to_string()
}

/// Résultats d'une recherche : Entrée va au dossier de l'élément et le sélectionne.
pub fn results_dialog(w: &Rc<Win>, p: &Rc<Pane>, title: &str, cwd: String, items: Vec<String>) {
    let (d, list, _) = list_dialog(title, 720, "Entrée : y aller   ·   Échap : fermer");
    for it in &items {
        let r = adw::ActionRow::new();
        r.set_title(&esc(it));
        r.set_activatable(true);
        list.append(&r);
    }
    let (pw, dw) = (Rc::downgrade(p), d.downgrade());
    list.connect_row_activated(move |_, row| {
        let Some(rel) = items.get(row.index() as usize) else { return };
        let full = mysend::util::normpath(&join(&cwd, rel));
        if let Some(d) = dw.upgrade() {
            d.close();
        }
        if let Some(p) = pw.upgrade() {
            p.navigate_select(dirname(&full), basename(&full).to_string());
            p.focus_list();
        }
    });
    d.present(Some(&w.window));
    if let Some(r) = list.row_at_index(0) {
        r.grab_focus();
    }
}

impl Win {
    /// ' : marque-pages ; leur lettre (ou Entrée) y va, Suppr en efface un.
    pub fn bookmarks_dialog(self: &Rc<Self>) {
        let keys: Vec<String> = self.bookmarks.borrow().keys().cloned().collect();
        let (d, list, _) = list_dialog("Marque-pages", 640, "lettre ou Entrée : y aller   ·   Suppr : effacer   ·   nouveau : m puis une lettre");
        if keys.is_empty() {
            let r = adw::ActionRow::new();
            r.set_title("aucun marque-page");
            list.append(&r);
        }
        for k in &keys {
            let b = self.bookmarks.borrow().get(k).cloned().unwrap();
            let r = adw::ActionRow::new();
            r.set_title(&esc(&format!("{}:{}", b.label, b.path)));
            let l = gtk::Label::new(Some(k));
            l.add_css_class("key-hint");
            l.set_width_chars(2);
            r.add_prefix(&l);
            r.set_activatable(true);
            list.append(&r);
        }
        let go = {
            let dw = d.downgrade();
            move |k: &str| {
                let w = win();
                let Some(b) = w.bookmarks.borrow().get(k).cloned() else { return false };
                if let Some(d) = dw.upgrade() {
                    d.close();
                }
                w.goto(b.target, b.label, Some(b.path));
                true
            }
        };
        let (k2, go2) = (keys.clone(), go.clone());
        list.connect_row_activated(move |_, row| {
            if let Some(k) = k2.get(row.index() as usize) {
                go2(k);
            }
        });
        let ctl = gtk::EventControllerKey::new();
        ctl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let (lw, dw) = (list.downgrade(), d.downgrade());
        ctl.connect_key_pressed(move |_, key, _, state| {
            if !state.is_empty() && state != gdk::ModifierType::SHIFT_MASK {
                return glib::Propagation::Proceed;
            }
            if matches!(key, gdk::Key::Delete | gdk::Key::KP_Delete) {
                let idx = lw.upgrade().and_then(|l| l.selected_row()).map(|r| r.index() as usize);
                if let Some(k) = idx.and_then(|i| keys.get(i)) {
                    let w = win();
                    w.bookmarks.borrow_mut().remove(k);
                    bookmarks_save(&w);
                    if let Some(d) = dw.upgrade() {
                        d.close();
                    }
                    w.bookmarks_dialog();
                }
                return glib::Propagation::Stop;
            }
            match key.to_unicode() {
                Some(c) if c.is_alphanumeric() && go(&c.to_string()) => glib::Propagation::Stop,
                _ => glib::Propagation::Proceed,
            }
        });
        d.add_controller(ctl);
        d.present(Some(&self.window));
        if let Some(r) = list.row_at_index(0) {
            r.grab_focus();
        }
    }

    /// T : corbeille de la machine de l'onglet : restaurer, supprimer, vider.
    pub fn trash_dialog(self: &Rc<Self>, p: &Rc<Pane>) {
        let fs = p.fs.borrow().clone();
        let (d, list, col) = list_dialog(&format!("Corbeille — {}", fs.label), 760, "Échap : fermer");
        let empty = gtk::Button::with_label("Vider la corbeille");
        empty.add_css_class("destructive-action");
        empty.set_halign(gtk::Align::End);
        col.append(&empty);
        let loading = adw::ActionRow::new();
        loading.set_title("Lecture de la corbeille…");
        list.append(&loading);
        let (lw, fs2) = (list.downgrade(), fs.clone());
        fill_trash(lw.clone(), fs.clone());
        let dw = d.downgrade();
        empty.connect_clicked(move |_| {
            let (fs3, lw2) = (fs2.clone(), lw.clone());
            let Some(d) = dw.upgrade() else { return };
            crate::dialogs::confirm(&d, "Vider la corbeille ?", &format!("Tout sera supprimé définitivement sur {}.", fs2.label), "Vider", true, move || {
                let fs4 = fs3.clone();
                bg(
                    move || trash::empty(&fs4).map_err(|e| errstr(&e)),
                    move |r| {
                        if let Err(e) = r {
                            win().error(&e);
                        }
                        fill_trash(lw2, fs3);
                    },
                );
            });
        });
        d.present(Some(&self.window));
    }

    pub fn enqueue(&self, task: mysend::transfers::Task) {
        mysend::transfers::enqueue(&self.transfers, task, |d| {
            crate::util::on_main(move || {
                let w = win();
                mysend::notify::transfer_done(&d);
                match d.result {
                    Ok(()) => w.toast(&format!("Terminé : {}", d.title)),
                    Err(msg) => w.error(&format!("{} : {msg}", d.title)),
                }
                w.history.borrow_mut().push(mysend::undo::UndoOp::Batch(d.undo));
                w.refresh_fs(&d.touches);
            })
        });
    }
}

fn bookmarks_save(w: &Win) {
    mysend::bookmarks::save(&w.bookmarks.borrow());
    w.rebuild_sidebar();
}

/// (Re)remplit la liste de la corbeille.
fn fill_trash(lw: glib::WeakRef<gtk::ListBox>, fs: FsRef) {
    let fs2 = fs.clone();
    bg(
        move || trash::list(&fs2).map_err(|e| errstr(&e)),
        move |r| {
            let Some(list) = lw.upgrade() else { return };
            while let Some(c) = list.first_child() {
                list.remove(&c);
            }
            let items: Vec<TrashItem> = match r {
                Ok(v) => v,
                Err(e) => {
                    let row = adw::ActionRow::new();
                    row.set_title(&esc(&e));
                    list.append(&row);
                    return;
                }
            };
            if items.is_empty() {
                let row = adw::ActionRow::new();
                row.set_title("La corbeille est vide.");
                list.append(&row);
                return;
            }
            for it in items {
                let row = adw::ActionRow::new();
                row.set_title(&esc(&it.name));
                row.set_subtitle(&esc(&format!("{}  ·  {}", it.path, it.date)));
                let restore = gtk::Button::from_icon_name("edit-undo-symbolic");
                restore.set_tooltip_text(Some("Restaurer"));
                restore.add_css_class("flat");
                restore.set_valign(gtk::Align::Center);
                let del = gtk::Button::from_icon_name("user-trash-full-symbolic");
                del.set_tooltip_text(Some("Supprimer définitivement"));
                del.add_css_class("flat");
                del.set_valign(gtk::Align::Center);
                row.add_suffix(&restore);
                row.add_suffix(&del);
                list.append(&row);
                for (btn, restore) in [(restore, true), (del, false)] {
                    let (fs, it, lw) = (fs.clone(), it.clone(), lw.clone());
                    btn.connect_clicked(move |_| {
                        let (fs2, it2, fs3, lw2) = (fs.clone(), it.clone(), fs.clone(), lw.clone());
                        bg(
                            move || if restore { trash::restore(&fs2, &it2).map(Some) } else { trash::delete(&fs2, &it2).map(|_| None) }.map_err(|e| errstr(&e)),
                            move |r| {
                                let w = win();
                                match r {
                                    Ok(Some(dest)) => w.toast(&format!("Restauré : {dest}")),
                                    Ok(None) => {}
                                    Err(e) => w.error(&e),
                                }
                                w.refresh_fs(&[fs3.clone()]);
                                fill_trash(lw2, fs3);
                            },
                        );
                    });
                }
            }
        },
    );
}
