//! Comparaison et synchronisation de dossiers / fichiers entre deux onglets, fusion.

use crate::app::App;
use crate::config::mkdtemp;
use crate::fs::errstr;
use crate::modals::{DirCmp, FileCmp, ModalKind};
use crate::style::{b, bold, fg, CYAN, GREEN, RED};
use crate::util::{basename, fmt_time, human};
use ratatui::text::{Line, Span};

pub use crate::compare::{Itemized, Side};
use crate::compare::{self, DiffLine, DirSync};

impl App {
    pub fn compare_dirs(&mut self, n: usize) {
        if n >= self.tabs.len() || n == self.active {
            self.warn("=N : compare avec l'onglet N (un autre que celui-ci).");
            return;
        }
        let (a, b) = (&self.tabs[self.active], &self.tabs[n]);
        if let Err(e) = compare::check_dirs(&a.fs, &b.fs) {
            self.warn(&e);
            return;
        }
        let m = DirCmp { here: (a.fs.clone(), a.cwd.clone()), there: (b.fs.clone(), b.cwd.clone()), result: None };
        let (fa, pa, fb, pb) = (a.fs.clone(), a.cwd.clone(), b.fs.clone(), b.cwd.clone());
        let id = self.push_modal(ModalKind::DirCmp(m));
        self.send_thread(move || {
            let res = compare::compare_dirs(&fa, &pa, &fb, &pb);
            Box::new(move |app: &mut App| {
                if let Some(ModalKind::DirCmp(m)) = app.modal_mut(id) {
                    m.result = Some(res);
                }
            })
        });
    }

    /// Action d'une touche dans la fenêtre de comparaison de dossiers.
    pub fn dir_sync(&mut self, m: &DirCmp, key: &str) {
        let (fa, pa) = m.here.clone();
        let (fb, pb) = m.there.clone();
        let kind = match key {
            ">" => DirSync::Send,
            "<" => DirSync::Fetch,
            "u" => DirSync::Newest,
            "m" => DirSync::Mirror,
            _ => return,
        };
        let task = compare::dir_sync_task(fa, pa, fb, pb, kind);
        self.enqueue(task);
    }

    pub fn compare_files(&mut self, n: usize) {
        if n >= self.tabs.len() || n == self.active {
            self.warn("-N : compare avec le fichier sous le curseur de l'onglet N.");
            return;
        }
        let side_of = |t: &crate::app::Tab| {
            t.current().filter(|e| !e.is_dir).map(|e| Side { fs: t.fs.clone(), path: t.path_of(&e.name), entry: e.clone() })
        };
        let (Some(a), Some(b)) = (side_of(&self.tabs[self.active]), side_of(&self.tabs[n])) else {
            self.warn("Place le curseur sur un fichier dans chacun des deux onglets.");
            return;
        };
        let base_date = compare::base_date(&a.fs, &a.path, &b.fs, &b.path);
        let (sa, sb) = (a.clone(), b.clone());
        let id = self.push_modal(ModalKind::FileCmp(FileCmp { a, b, base_date, content: None }));
        self.send_thread(move || {
            let lines = diff_lines(compare::diff_files(&sa, &sb));
            Box::new(move |app: &mut App| {
                if let Some(ModalKind::FileCmp(m)) = app.modal_mut(id) {
                    m.content = Some(lines);
                }
            })
        });
    }

    /// Copie d'un fichier d'un côté vers l'autre (tâche), puis enregistre l'ancêtre.
    pub fn file_copy(&mut self, src: &Side, dst: &Side) {
        let task = compare::file_copy_task(src, dst);
        self.enqueue(task);
    }

    pub fn file_merge(&mut self, a: Side, b: Side) {
        let m = match compare::prepare_merge(&a, &b) {
            Ok(m) => m,
            Err(e) => return self.warn(&e),
        };
        let tmp = match mkdtemp("merge-") {
            Ok(t) => t,
            Err(e) => return self.error(&errstr(&e)),
        };
        let (result, conflicts, is_three) = (m.text, m.conflicts, m.three);
        let out = tmp.join(basename(&a.path));
        if let Err(e) = std::fs::write(&out, &result) {
            let _ = std::fs::remove_dir_all(&tmp);
            return self.error(&errstr(&e));
        }
        self.run_editor(&[out.to_string_lossy().into_owned()], false);
        let edited = std::fs::read(&out).unwrap_or_else(|_| result.into_bytes());
        let markers = compare::remaining_conflicts(&edited);
        let msg = format!(
            "Fusion {} : {conflicts} conflit(s) au départ.\nÉcrire le résultat sur {} et sur {} ?",
            if is_three { "à trois (avec l'ancêtre commun)" } else { "à deux" },
            a.label(),
            b.label()
        );
        let extra = if markers > 0 { Some(format!("Il reste des marqueurs de conflit ({markers}).")) } else { None };
        let tmp2 = tmp.clone();
        let touched = vec![a.fs.clone(), b.fs.clone()];
        let touched2 = touched.clone();
        self.confirm_full(
            &msg,
            extra,
            Box::new(move |app| {
                let changed: Vec<String> = [&a, &b].iter().filter(|s| s.changed()).map(|s| s.fs.label.clone()).collect();
                let tmp3 = tmp.clone();
                let t3 = touched.clone();
                let write = move |app: &mut App| {
                    match compare::write_merge(&a, &b, &edited) {
                        Ok(()) => app.info(&format!("Fusion enregistrée sur {} et {}.", a.label(), b.label())),
                        Err(e) => app.error(&e),
                    }
                    let _ = std::fs::remove_dir_all(&tmp);
                    app.refresh_fs(&touched);
                };
                if changed.is_empty() {
                    write(app);
                } else {
                    app.confirm_full(
                        &format!("Le fichier a changé sur {} pendant la fusion. Écraser quand même ?", changed.join(" et ")),
                        None,
                        Box::new(write),
                        Some(Box::new(move |app| {
                            app.info("Fusion abandonnée : rien n'a été modifié.");
                            let _ = std::fs::remove_dir_all(&tmp3);
                            app.refresh_fs(&t3);
                        })),
                    );
                }
            }),
            Some(Box::new(move |app| {
                app.info("Fusion abandonnée : rien n'a été modifié.");
                let _ = std::fs::remove_dir_all(&tmp2);
                app.refresh_fs(&touched2);
            })),
        );
    }
}

fn diff_lines(d: Vec<DiffLine>) -> Vec<Line<'static>> {
    d.into_iter()
        .map(|l| match l {
            DiffLine::Header(t) => Line::from(Span::styled(t, b())),
            DiffLine::Hunk(t) => Line::from(Span::styled(t, fg(CYAN))),
            DiffLine::Del(t) => Line::from(Span::styled(t, fg(RED))),
            DiffLine::Add(t) | DiffLine::Ok(t) => Line::from(Span::styled(t, fg(GREEN))),
            DiffLine::Same(t) | DiffLine::Note(t) => Line::from(t),
        })
        .collect()
}

pub fn file_row(tag: &str, w: usize, s: &Side, newer: bool) -> Line<'static> {
    let mut spans = vec![
        Span::styled(format!("{tag:<w$}"), b()),
        Span::raw(format!("{}  ", s.label())),
        Span::raw(format!("{}  ", human(s.entry.size))),
        Span::raw(fmt_time(s.entry.mtime, "%Y-%m-%d %H:%M:%S")),
    ];
    if newer {
        spans.push(Span::styled("  plus récent", bold(GREEN)));
    }
    Line::from(spans)
}
