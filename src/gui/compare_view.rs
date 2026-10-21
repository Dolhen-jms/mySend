//! Comparer, synchroniser (dossiers) ou fusionner (fichiers) deux onglets : l'autre panneau en
//! double panneau, sinon un autre onglet. Ctrl+K.

use crate::dialogs;
use crate::pane::Pane;
use crate::util::{bg, escape, in_terminal};
use crate::window::{win, Win};
use adw::prelude::*;
use mysend::compare::{self, DiffLine, DirSync, Itemized, Side};
use mysend::fs::FsRef;
use mysend::util::{basename, fmt_time, human};
use std::rc::Rc;

/// Fenêtre de base : en-tête, contenu défilant, rangée de boutons.
fn frame(title: &str, width: i32) -> (adw::Dialog, gtk::Box, gtk::Box, gtk::Box) {
    let d = adw::Dialog::new();
    d.set_title(title);
    d.set_content_width(width);
    d.set_content_height(640);
    let tb = adw::ToolbarView::new();
    tb.add_top_bar(&adw::HeaderBar::new());
    let col = gtk::Box::new(gtk::Orientation::Vertical, 10);
    col.set_margin_top(6);
    col.set_margin_bottom(14);
    col.set_margin_start(14);
    col.set_margin_end(14);
    let head = gtk::Box::new(gtk::Orientation::Vertical, 4);
    col.append(&head);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let sw = gtk::ScrolledWindow::builder().child(&body).vexpand(true).hscrollbar_policy(gtk::PolicyType::Automatic).build();
    col.append(&sw);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    col.append(&buttons);
    tb.set_content(Some(&col));
    d.set_child(Some(&tb));
    (d, head, body, buttons)
}

fn line(markup: &str) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_markup(markup);
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_selectable(true);
    l
}

fn button(label: &str, tip: &str, style: Option<&str>) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.set_tooltip_text(Some(tip));
    if let Some(s) = style {
        b.add_css_class(s);
    }
    b.set_sensitive(false);
    b
}

impl Win {
    /// Ctrl+K : deux fichiers sélectionnés (un de chaque côté) → comparaison de fichiers ;
    /// sinon comparaison des deux dossiers.
    pub fn compare(self: &Rc<Self>) {
        let Some(a) = self.current() else { return };
        if let Some(b) = self.other_pane() {
            self.compare_with(&a, &b);
            return;
        }
        let others: Vec<Rc<Pane>> = self.panes.borrow().iter().filter(|p| !Rc::ptr_eq(p, &a)).cloned().collect();
        match others.len() {
            0 => self.toast("Ouvre un deuxième onglet (ou le double panneau, F3) pour comparer."),
            1 => self.compare_with(&a, &others[0]),
            _ => {
                // choix de l'onglet à comparer
                let d = adw::AlertDialog::new(Some("Comparer avec…"), Some(&format!("{}:{}", a.fs.borrow().label, a.cwd.borrow())));
                for (i, o) in others.iter().enumerate() {
                    d.add_response(&i.to_string(), &format!("{}:{}", o.fs.borrow().label, o.fs.borrow().pretty(&o.cwd.borrow())));
                }
                d.add_response("cancel", "Annuler");
                d.set_close_response("cancel");
                d.set_prefer_wide_layout(true);
                let a2 = a.clone();
                d.connect_response(None, move |_, r| {
                    if let Some(o) = r.parse::<usize>().ok().and_then(|i| others.get(i)) {
                        win().compare_with(&a2, o);
                    }
                });
                d.present(Some(&self.window));
            }
        }
    }

    fn compare_with(self: &Rc<Self>, a: &Rc<Pane>, b: &Rc<Pane>) {
        let side = |p: &Rc<Pane>| -> Option<Side> {
            let sel = p.selected();
            let [(path, e)] = sel.as_slice() else { return None };
            (!e.is_dir).then(|| Side { fs: p.fs.borrow().clone(), path: path.clone(), entry: e.clone() })
        };
        match (side(a), side(b)) {
            (Some(x), Some(y)) => self.compare_files(x, y),
            _ => self.compare_dirs(a.fs.borrow().clone(), a.cwd.borrow().clone(), b.fs.borrow().clone(), b.cwd.borrow().clone()),
        }
    }

    // ---------------------------------------------------------------- dossiers

    fn compare_dirs(self: &Rc<Self>, fa: FsRef, pa: String, fb: FsRef, pb: String) {
        if let Err(e) = compare::check_dirs(&fa, &fb) {
            self.error(&e);
            return;
        }
        let (d, head, body, buttons) = frame("Comparer les dossiers", 820);
        head.append(&line(&format!("<b>ici</b>       {}", escape(&format!("{}:{}", fa.label, pa)))));
        head.append(&line(&format!("<b>là-bas</b>   {}", escape(&format!("{}:{}", fb.label, pb)))));
        let spinner = adw::Spinner::new();
        spinner.set_size_request(32, 32);
        body.append(&spinner);
        let send = button("Envoyer ici → là-bas", "Ajoute et remplace là-bas, ne supprime rien", Some("suggested-action"));
        let fetch = button("Récupérer là-bas → ici", "Ajoute et remplace ici, ne supprime rien", None);
        let newest = button("Le plus récent gagne", "Dans les deux sens : chaque fichier garde sa version la plus récente", None);
        let mirror = button("Miroir ici → là-bas…", "Là-bas devient identique à ici (supprime ce qui est en trop là-bas)", Some("destructive-action"));
        for b in [&mirror, &newest, &fetch, &send] {
            buttons.append(b);
        }
        let run = {
            let (fa, pa, fb, pb, dw) = (fa.clone(), pa.clone(), fb.clone(), pb.clone(), d.downgrade());
            move |kind: DirSync| {
                if let Some(d) = dw.upgrade() {
                    d.close();
                }
                win().enqueue(compare::dir_sync_task(fa.clone(), pa.clone(), fb.clone(), pb.clone(), kind));
            }
        };
        for (b, kind) in [(&send, DirSync::Send), (&fetch, DirSync::Fetch), (&newest, DirSync::Newest)] {
            let run = run.clone();
            b.connect_clicked(move |_| run(kind));
        }
        d.present(Some(&self.window));

        let (fa2, pa2, fb2, pb2) = (fa.clone(), pa.clone(), fb.clone(), pb.clone());
        let bw = body.downgrade();
        let dw = d.downgrade();
        bg(
            move || compare::compare_dirs(&fa2, &pa2, &fb2, &pb2),
            move |r: Result<Itemized, String>| {
                let Some(body) = bw.upgrade() else { return };
                while let Some(c) = body.first_child() {
                    body.remove(&c);
                }
                let it = match r {
                    Ok(it) => it,
                    Err(e) => {
                        body.append(&line(&format!("<span foreground=\"{}\">{}</span>", crate::theme::current().red, escape(&e))));
                        return;
                    }
                };
                if it.is_empty() {
                    body.append(&line(&format!("<span foreground=\"{}\"><b>Identiques.</b></span>", crate::theme::current().green)));
                    return;
                }
                let p = crate::theme::current();
                for (title, v, color) in [
                    ("Seulement ici", &it.only_here, &p.green),
                    ("Différents (taille ou date)", &it.differ, &p.yellow),
                    ("Seulement là-bas", &it.only_there, &p.red),
                ] {
                    if v.is_empty() {
                        continue;
                    }
                    body.append(&line(&format!("<b><span foreground=\"{color}\">{title}</span></b>  ({})", v.len())));
                    let mut names: Vec<String> = v.iter().take(400).map(|n| escape(n)).collect();
                    if v.len() > 400 {
                        names.push(format!("<i>… et {} autres</i>", v.len() - 400));
                    }
                    let l = line(&names.join("\n"));
                    l.add_css_class("preview-text");
                    body.append(&l);
                }
                for b in [&send, &fetch, &newest, &mirror] {
                    b.set_sensitive(true);
                }
                let n = it.only_there.len();
                let run = run.clone();
                let dw = dw.clone();
                mirror.connect_clicked(move |_| {
                    let Some(d) = dw.upgrade() else { return };
                    let run = run.clone();
                    let msg = if n > 0 {
                        format!("{n} élément(s) qui n'existent que là-bas seront supprimés là-bas.")
                    } else {
                        "Rien ne sera supprimé là-bas.".to_string()
                    };
                    dialogs::confirm(&d, "Miroir ici → là-bas ?", &msg, "Miroir", true, move || run(DirSync::Mirror));
                });
            },
        );
    }

    // ---------------------------------------------------------------- fichiers

    fn compare_files(self: &Rc<Self>, a: Side, b: Side) {
        let (d, head, body, buttons) = frame("Comparer les fichiers", 900);
        let p = crate::theme::current();
        let row = |tag: &str, s: &Side, newer: bool| {
            let mut m = format!(
                "<b>{tag}</b>   {}   {}   {}",
                escape(&s.label()),
                human(s.entry.size),
                fmt_time(s.entry.mtime, "%Y-%m-%d %H:%M:%S")
            );
            if newer {
                m.push_str(&format!("   <span foreground=\"{}\"><b>plus récent</b></span>", p.green));
            }
            line(&m)
        };
        head.append(&row("ici      ", &a, a.entry.mtime > b.entry.mtime));
        head.append(&row("là-bas", &b, b.entry.mtime > a.entry.mtime));
        let base = match compare::base_date(&a.fs, &a.path, &b.fs, &b.path) {
            Some(t) => format!("Ancêtre commun connu (dernière synchro par mySend le {}) : la fusion règle seule ce qui n'a changé que d'un côté.", fmt_time(t, "%Y-%m-%d %H:%M")),
            None => "Ancêtre commun inconnu (jamais synchronisés par mySend) : à la fusion, chaque différence sera un conflit à trancher.".into(),
        };
        let bl = line(&format!("<small>{}</small>", escape(&base)));
        bl.add_css_class("muted");
        head.append(&bl);
        let spinner = adw::Spinner::new();
        spinner.set_size_request(32, 32);
        body.append(&spinner);
        let to_b = button("Copier ici → là-bas", "Remplace le fichier de là-bas", Some("suggested-action"));
        let to_a = button("Copier là-bas → ici", "Remplace le fichier d'ici", None);
        let merge = button("Fusionner…", "Fusion automatique, puis les conflits restants à trancher dans l'éditeur", None);
        for b in [&merge, &to_a, &to_b] {
            buttons.append(b);
        }
        let dw = d.downgrade();
        for (btn, src, dst) in [(&to_b, a.clone(), b.clone()), (&to_a, b.clone(), a.clone())] {
            let dw = dw.clone();
            btn.connect_clicked(move |_| {
                if let Some(d) = dw.upgrade() {
                    d.close();
                }
                win().enqueue(compare::file_copy_task(&src, &dst));
            });
        }
        let (a2, b2) = (a.clone(), b.clone());
        let dw2 = dw.clone();
        merge.connect_clicked(move |_| {
            if let Some(d) = dw2.upgrade() {
                d.close();
            }
            win().merge(a2.clone(), b2.clone());
        });
        d.present(Some(&self.window));

        let bw = body.downgrade();
        bg(
            move || compare::diff_files(&a, &b),
            move |lines| {
                let Some(body) = bw.upgrade() else { return };
                while let Some(c) = body.first_child() {
                    body.remove(&c);
                }
                let p = crate::theme::current();
                let identical = matches!(lines.as_slice(), [DiffLine::Ok(_)]);
                let text: Vec<String> = lines
                    .into_iter()
                    .take(5000)
                    .map(|l| match l {
                        DiffLine::Header(t) => format!("<b>{}</b>", escape(&t)),
                        DiffLine::Hunk(t) => format!("<span foreground=\"{}\">{}</span>", p.blue, escape(&t)),
                        DiffLine::Del(t) => format!("<span foreground=\"{}\">{}</span>", p.red, escape(&t)),
                        DiffLine::Add(t) | DiffLine::Ok(t) => format!("<span foreground=\"{}\">{}</span>", p.green, escape(&t)),
                        DiffLine::Same(t) | DiffLine::Note(t) => escape(&t),
                    })
                    .collect();
                let l = line(&text.join("\n"));
                l.set_wrap(false);
                l.add_css_class("preview-text");
                body.append(&l);
                if !identical {
                    for b in [&to_b, &to_a, &merge] {
                        b.set_sensitive(true);
                    }
                }
            },
        );
    }

    /// Fusion : automatique, puis relue dans l'éditeur (dans un terminal), puis écrite des deux côtés.
    fn merge(self: &Rc<Self>, a: Side, b: Side) {
        self.toast("Fusion…");
        let (a2, b2) = (a.clone(), b.clone());
        bg(
            move || {
                let m = compare::prepare_merge(&a2, &b2)?;
                let tmp = mysend::config::mkdtemp("merge-").map_err(|e| e.to_string())?;
                let file = tmp.join(basename(&a2.path));
                std::fs::write(&file, &m.text).map_err(|e| e.to_string())?;
                // l'éditeur dans un terminal ; on attend qu'il soit fermé
                let ed = mysend::config::editor();
                let mut argv = shlex::split(&ed).unwrap_or_else(|| vec![ed.clone()]);
                argv.push(file.to_string_lossy().into_owned());
                let cmd = in_terminal(&argv);
                std::process::Command::new(&cmd[0]).args(&cmd[1..]).status().map_err(|e| format!("{} : {e}", cmd[0]))?;
                let edited = std::fs::read(&file).map_err(|e| e.to_string())?;
                let _ = std::fs::remove_dir_all(&tmp);
                Ok::<_, String>((m, edited))
            },
            move |r| {
                let w = win();
                let (m, edited) = match r {
                    Ok(x) => x,
                    Err(e) => return w.error(&e),
                };
                let left = compare::remaining_conflicts(&edited);
                let mut body = format!(
                    "Fusion {} : {} conflit(s) au départ.\nÉcrire le résultat sur {} et sur {} ?",
                    if m.three { "à trois (avec l'ancêtre commun)" } else { "à deux" },
                    m.conflicts,
                    a.label(),
                    b.label()
                );
                if left > 0 {
                    body.push_str(&format!("\n\nAttention : il reste {left} marqueur(s) de conflit (<<<<<<< >>>>>>>)."));
                }
                dialogs::confirm(&w.window, "Enregistrer la fusion ?", &body, "Écrire des deux côtés", left > 0, move || {
                    let (a3, b3) = (a.clone(), b.clone());
                    bg(
                        move || {
                            let changed: Vec<String> = [&a3, &b3].iter().filter(|s| s.changed()).map(|s| s.fs.label.clone()).collect();
                            if !changed.is_empty() {
                                return Err(format!("Le fichier a changé sur {} pendant la fusion : rien n'a été écrit.", changed.join(" et ")));
                            }
                            compare::write_merge(&a3, &b3, &edited).map(|_| (a3.fs.clone(), b3.fs.clone()))
                        },
                        |r| {
                            let w = win();
                            match r {
                                Ok((fa, fb)) => {
                                    w.toast("Fusion enregistrée des deux côtés.");
                                    w.refresh_fs(&[fa, fb]);
                                }
                                Err(e) => w.error(&e),
                            }
                        },
                    );
                });
            },
        );
    }
}
