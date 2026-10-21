//! Opérations sur les fichiers : presse-papiers, collage, suppression, création, renommage,
//! archives, édition, ouverture, shell, recherches.

use crate::app::{App, Severity, Yank};
use crate::config::{editor, mkdtemp};
use crate::fs::{errstr, same, FsRef};
use crate::modals::ModalKind;
use crate::transfers;
use crate::util::{basename, dirname, is_within, join, normpath, relpath, sha256_hex};
use std::collections::VecDeque;
use std::path::PathBuf;

fn first_component(path: &str, cwd: &str) -> String {
    relpath(path, cwd).split('/').next().unwrap_or("").to_string()
}

pub struct EditItem {
    pub fs: FsRef,
    pub path: String,
    pub local: PathBuf,
    pub hash: String,
    pub stamp: (u64, u64),
}

impl App {
    // ------------------------------------------------------------ presse-papiers

    pub fn yank(&mut self, cut: bool) {
        let fs = self.tab().fs.clone();
        let items = self.tab_mut().targets();
        if items.is_empty() {
            return;
        }
        self.yank = Some(Yank { fs, items: items.into_iter().collect(), cut });
        for t in &mut self.tabs {
            t.selected.clear();
        }
    }

    pub fn paste(&mut self, overwrite: bool) {
        let Some(y) = &self.yank else {
            self.warn("Presse-papiers vide : y pour copier, x pour couper.");
            return;
        };
        let (sfs, items, cut) = (y.fs.clone(), y.items.clone(), y.cut);
        let dfs = self.tab().fs.clone();
        let ddir = self.tab().cwd.clone();
        let n = items.len();
        let title = format!(
            "{} de {n} élément(s) de {} vers {}:{}",
            if cut { "Déplacement" } else { "Copie" },
            sfs.label,
            dfs.label,
            ddir
        );
        let task = transfers::paste_task(title, sfs, items.into_iter().collect(), cut, overwrite, dfs, ddir);
        self.enqueue(task);
        if cut {
            self.yank = None;
        }
    }

    // ------------------------------------------------------------ suppression

    pub fn delete(&mut self, definitive: bool) {
        let fs = self.tab().fs.clone();
        let items = self.tab_mut().targets();
        if items.is_empty() {
            return;
        }
        let what = if items.len() == 1 { format!("« {} »", items[0].1.name) } else { format!("{} éléments", items.len()) };
        let msg = if definitive {
            format!("Supprimer **définitivement** {what} sur {} ?", fs.label)
        } else {
            format!("Mettre {what} à la corbeille sur {} ?", fs.label)
        };
        self.confirm(&msg, move |app| {
            let paths: Vec<String> = items.into_iter().map(|(p, _)| p).collect();
            let fs2 = fs.clone();
            app.send_thread(move || {
                let mut errs = Vec::new();
                let mut done = Vec::new();
                for p in &paths {
                    let r = if definitive { fs2.remove(p) } else { crate::trash::put(&fs2, p) };
                    match r {
                        Err(e) => errs.push(format!("{} : {}", basename(p), errstr(&e))),
                        Ok(()) if !definitive => done.push(crate::undo::UndoOp::Trash { fs: fs2.clone(), original: p.clone() }),
                        Ok(()) => {}
                    }
                }
                Box::new(move |app: &mut App| {
                    app.history.push(crate::undo::UndoOp::Batch(done));
                    if !errs.is_empty() {
                        app.notify_t(&errs.join("\n"), "Suppression", Severity::Error, 10.0);
                    }
                    for p in &paths {
                        for t in &mut app.tabs {
                            t.selected.remove(p);
                        }
                        if let Some(y) = &mut app.yank {
                            if same(&y.fs, &fs2) {
                                y.items.remove(p);
                            }
                        }
                    }
                    if app.yank.as_ref().map(|y| y.items.is_empty()).unwrap_or(false) {
                        app.yank = None;
                    }
                    app.refresh_fs(&[fs2]);
                })
            });
        });
    }

    // ------------------------------------------------------------ créer, renommer

    pub fn create(&mut self) {
        self.prompt("Créer (terminer par « / » pour un dossier) :", "", None, |app, v| {
            let Some(v) = v else { return };
            let name = v.trim().to_string();
            if name.is_empty() {
                return;
            }
            let (fs, cwd) = (app.tab().fs.clone(), app.tab().cwd.clone());
            let path = normpath(&join(&cwd, &name));
            let r = if fs.exists(&path) {
                Err(std::io::Error::other("existe déjà"))
            } else if name.ends_with('/') {
                fs.makedirs(&path)
            } else {
                fs.makedirs(&dirname(&path)).and_then(|_| fs.touch(&path))
            };
            match r {
                Ok(()) => {
                    // un dossier créé avec ses parents : on n'annule que le dernier
                    app.history.push(crate::undo::UndoOp::Create { fs: fs.clone(), path: path.clone() });
                    fs.clear_cache();
                    app.reload(Some(first_component(&path, &cwd)));
                }
                Err(e) => app.error(&format!("{name} : {}", errstr(&e))),
            }
        });
    }

    pub fn rename(&mut self) {
        if self.tab().current().is_none() {
            return;
        }
        self.tab_mut().commit_visual();
        if self.tab().selected.len() > 1 {
            self.rename_batch();
            return;
        }
        let e = self.tab().current().cloned().unwrap();
        let cursor = match e.name.rfind('.') {
            Some(i) if i > 0 && !e.is_dir => Some(e.name[..i].chars().count()),
            _ => None,
        };
        let src = self.tab().path_of(&e.name);
        self.prompt("Renommer :", &e.name.clone(), cursor, move |app, v| {
            let Some(v) = v else { return };
            let new = v.trim().to_string();
            if new.is_empty() || new == e.name {
                return;
            }
            let (fs, cwd) = (app.tab().fs.clone(), app.tab().cwd.clone());
            let dst = normpath(&join(&cwd, &new));
            match fs.rename(&src, &dst) {
                Ok(()) => {
                    app.history.push(crate::undo::UndoOp::Rename { fs: fs.clone(), from: src.clone(), to: dst.clone() });
                    app.tab_mut().selected.remove(&src);
                    fs.clear_cache();
                    app.reload(Some(first_component(&dst, &cwd)));
                }
                Err(err) => app.error(&format!("{new} : {}", errstr(&err))),
            }
        });
    }

    fn rename_batch(&mut self) {
        let (fs, cwd) = (self.tab().fs.clone(), self.tab().cwd.clone());
        let paths: Vec<String> = self.tab().selected.keys().cloned().collect();
        let lines: Vec<String> = paths
            .iter()
            .map(|p| if is_within(p, &cwd) && p != &cwd { relpath(p, &cwd) } else { p.clone() })
            .collect();
        let tmp = match mkdtemp("rename-") {
            Ok(t) => t,
            Err(e) => return self.error(&errstr(&e)),
        };
        let file = tmp.join("renommer.txt");
        if let Err(e) = std::fs::write(&file, lines.join("\n") + "\n") {
            let _ = std::fs::remove_dir_all(&tmp);
            return self.error(&errstr(&e));
        }
        let ok = self.run_editor(&[file.to_string_lossy().into_owned()], false);
        if ok {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let new: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
            if new.len() != lines.len() {
                self.notify(
                    &format!("{} ligne(s) au lieu de {} : rien n'a été renommé.", new.len(), lines.len()),
                    Severity::Warning,
                    8.0,
                );
            } else {
                let pairs: Vec<(String, String)> = paths
                    .iter()
                    .zip(lines.iter().zip(new.iter()))
                    .filter(|(_, (old, n))| !n.trim().is_empty() && n != old)
                    .map(|(p, (_, n))| (p.clone(), normpath(&join(&cwd, n))))
                    .collect();
                match fs.rename_all(&pairs) {
                    Ok(()) => {
                        let ops = pairs.iter().map(|(a, b)| crate::undo::UndoOp::Rename { fs: fs.clone(), from: a.clone(), to: b.clone() }).collect();
                        self.history.push(crate::undo::UndoOp::Batch(ops));
                        self.info(&format!("{} élément(s) renommé(s).", pairs.len()));
                        self.tab_mut().selected.clear();
                    }
                    Err(e) => self.error(&format!("Renommage en lot : {}", errstr(&e))),
                }
            }
            self.refresh_fs(&[fs]);
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ------------------------------------------------------------ archives

    pub fn compress(&mut self) {
        let (fs, cwd) = (self.tab().fs.clone(), self.tab().cwd.clone());
        let items = self.tab_mut().targets();
        if items.is_empty() {
            return;
        }
        let base = if items.len() == 1 {
            items[0].1.name.clone()
        } else {
            let b = basename(&cwd);
            if b.is_empty() { "archive".into() } else { b.to_string() }
        };
        let default = format!("{base}.tar.gz");
        let cur = base.chars().count();
        self.prompt("Nom de l'archive (.tar.gz, .tar.xz, .tar.zst, .zip…) :", &default, Some(cur), move |app, v| {
            let Some(v) = v else { return };
            let name = v.trim().to_string();
            if name.is_empty() {
                return;
            }
            let out = normpath(&join(&cwd, &name));
            let task = crate::archive::compress_task(fs.clone(), cwd.clone(), &items, out);
            app.enqueue(task);
            app.tab_mut().selected.clear();
        });
    }

    pub fn extract(&mut self) {
        let t = self.tab();
        let Some(e) = t.current().cloned() else { return };
        if e.is_dir {
            return;
        }
        if crate::archive::archive_ext(&e.name).is_none() {
            self.warn("Pas une archive reconnue (tar, zip, 7z, gz, xz, bz2, zst).");
            return;
        }
        let task = crate::archive::extract_task(t.fs.clone(), t.cwd.clone(), &e.name);
        self.enqueue(task);
    }

    // ------------------------------------------------------------ ouvrir, éditer

    pub fn open_file(&mut self, path: String) {
        let fs = self.tab().fs.clone();
        match fs.read_head(&path, 8192) {
            Ok(None) => self.warn("Pas un fichier ordinaire."),
            Ok(Some(d)) if d.contains(&0) => self.xdg_open(path, false),
            Ok(Some(_)) => self.edit(vec![(fs, path)], false),
            Err(e) => self.error(&format!("{path} : {}", errstr(&e))),
        }
    }

    /// Lance l'éditeur en suspendant la TUI. Faux si l'éditeur est introuvable.
    pub fn run_editor(&mut self, files: &[String], split: bool) -> bool {
        let ed = editor();
        let mut argv = shlex::split(&ed).unwrap_or_else(|| vec![ed.clone()]);
        if argv.is_empty() {
            argv.push("hx".into());
        }
        if split && files.len() > 1 {
            match basename(&argv[0]) {
                "hx" | "helix" => argv.push("--vsplit".into()),
                "vim" | "nvim" => argv.push("-O".into()),
                _ => {}
            }
        }
        argv.extend(files.iter().cloned());
        let r = crate::tui::suspend(self, || std::process::Command::new(&argv[0]).args(&argv[1..]).status());
        match r {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.error(&format!("Éditeur introuvable : {ed} (variables MYSEND_EDITOR, VISUAL ou EDITOR)"));
                false
            }
            _ => true,
        }
    }

    pub fn edit(&mut self, files: Vec<(FsRef, String)>, split: bool) {
        let names: Vec<String> = files.iter().map(|(fs, p)| format!("{}:{}", fs.label, fs.pretty(p))).collect();
        crate::log::info(&format!("éditeur : {}", names.join(", ")));
        let mut tmp: Option<PathBuf> = None;
        let mut items = Vec::new();
        let mut locals = Vec::new();
        let mut touched: Vec<FsRef> = Vec::new();
        for (fs, path) in files {
            if !touched.iter().any(|f| same(f, &fs)) {
                touched.push(fs.clone());
            }
            if !fs.remote {
                locals.push(path);
                continue;
            }
            let r = (|| -> std::io::Result<EditItem> {
                let st = fs.stat(&path)?;
                if tmp.is_none() {
                    tmp = Some(mkdtemp("edit-")?);
                }
                let sub = tmp.as_ref().unwrap().join(format!("{}", items.len()));
                std::fs::create_dir_all(&sub)?;
                let local = sub.join(basename(&path));
                fs.get(&path, &local)?;
                let hash = sha256_hex(&std::fs::read(&local)?);
                Ok(EditItem { fs: fs.clone(), path: path.clone(), local, hash, stamp: (st.mtime.to_bits(), st.size) })
            })();
            match r {
                Ok(it) => {
                    locals.push(it.local.to_string_lossy().into_owned());
                    items.push(it);
                }
                Err(e) => {
                    self.notify_t(&format!("{path} : {}", errstr(&e)), &fs.label, Severity::Error, 8.0);
                    if let Some(t) = tmp {
                        let _ = std::fs::remove_dir_all(t);
                    }
                    return;
                }
            }
        }
        self.run_editor(&locals, split);
        let changed: VecDeque<EditItem> = items
            .into_iter()
            .filter(|it| std::fs::read(&it.local).map(|d| sha256_hex(&d) != it.hash).unwrap_or(false))
            .collect();
        self.edit_finish(changed, tmp, false, touched);
    }

    fn edit_finish(&mut self, mut q: VecDeque<EditItem>, tmp: Option<PathBuf>, kept: bool, touched: Vec<FsRef>) {
        let Some(it) = q.pop_front() else {
            if !kept {
                if let Some(t) = tmp {
                    let _ = std::fs::remove_dir_all(t);
                }
            }
            self.refresh_fs(&touched);
            return;
        };
        let name = basename(&it.path).to_string();
        let now = it.fs.stat(&it.path).ok().map(|e| (e.mtime.to_bits(), e.size));
        let upload = move |app: &mut App, it: EditItem, q, tmp, kept: bool, touched| {
            match it.fs.put(&it.local, &it.path) {
                Ok(()) => {
                    app.notify(&format!("Enregistré sur {} : {}", it.fs.label, basename(&it.path)), Severity::Info, 3.0);
                    app.edit_finish(q, tmp, kept, touched);
                }
                Err(e) => {
                    app.notify(
                        &format!("Envoi impossible ({}).\nTa version est dans {}", errstr(&e), it.local.display()),
                        Severity::Error,
                        10.0,
                    );
                    app.edit_finish(q, tmp, true, touched);
                }
            }
        };
        if now.is_some() && now != Some(it.stamp) {
            let msg = format!("« {name} » a changé sur {} pendant l'édition. Écraser avec ta version ?", it.fs.label);
            let local = it.local.clone();
            let it = std::rc::Rc::new(std::cell::RefCell::new(Some((it, q, tmp, touched))));
            let it2 = it.clone();
            self.confirm_full(
                &msg,
                None,
                Box::new(move |app| {
                    if let Some((it, q, tmp, touched)) = it.borrow_mut().take() {
                        upload(app, it, q, tmp, kept, touched);
                    }
                }),
                Some(Box::new(move |app| {
                    app.notify(&format!("Ta version est gardée dans {}", local.display()), Severity::Warning, 10.0);
                    if let Some((_, q, tmp, touched)) = it2.borrow_mut().take() {
                        app.edit_finish(q, tmp, true, touched);
                    }
                })),
            );
        } else {
            upload(self, it, q, tmp, kept, touched);
        }
    }

    pub fn xdg_open(&mut self, path: String, is_dir: bool) {
        let fs = self.tab().fs.clone();
        if fs.remote {
            if is_dir {
                self.warn("Ouvrir un dossier distant : pas possible, copie-le d'abord.");
                return;
            }
            self.notify(&format!("Téléchargement de {}…", basename(&path)), Severity::Info, 2.0);
            self.send_thread(move || {
                let r = mkdtemp("open-").and_then(|d| {
                    let l = d.join(basename(&path));
                    fs.get(&path, &l).map(|_| l)
                });
                Box::new(move |app: &mut App| match r {
                    Ok(l) => app.spawn_open(&l.to_string_lossy()),
                    Err(e) => app.error(&format!("{path} : {}", errstr(&e))),
                })
            });
        } else {
            self.spawn_open(&path);
        }
    }

    fn spawn_open(&mut self, local: &str) {
        let open = crate::platform::OPEN;
        crate::log::info(&format!("{open} {local}"));
        if let Err(_e) = crate::proc::spawn_detached(&crate::platform::open_argv(local)) {
            self.error(&format!("{open} introuvable."));
        }
    }

    // ------------------------------------------------------------ shell

    /// `!` : shell dans le dossier courant, en plein écran.
    ///
    /// L'interface est suspendue et le shell prend tout le terminal (un vrai terminal : couleurs,
    /// copier-coller et programmes plein écran du terminal hôte) ; `exit` revient à mySend.
    /// Machine distante : `ssh -t` qui se place dans le dossier ; onglet sudo : `sudo -u`.
    /// Le terminal intégré par-dessus l'app (`term.rs`) est désactivé.
    pub fn shell(&mut self) {
        let t = self.tab();
        let (fs, path) = (t.fs.clone(), t.cwd.clone());
        let title = format!("{}:{}", fs.label, fs.pretty(&path));
        crate::log::info(&format!("shell {title}"));
        // commande à lancer (ssh / sudo / $SHELL) et dossier de départ côté local
        let (argv, cwd) = fs.shell_argv(&path);
        crate::tui::suspend(self, || {
            println!("\nmySend — shell sur {title}  (exit pour revenir)\n");
            let _ = std::process::Command::new(&argv[0]).args(&argv[1..]).current_dir(&cwd).status();
        });
        self.refresh_fs(&[fs]);
    }

    // ------------------------------------------------------------ filtre, recherche

    pub fn filter_prompt(&mut self) {
        let old = self.tab().filter.clone();
        let tid = self.tab().id;
        let old2 = old.clone();
        let id = self.prompt("Filtrer :", &old, None, move |app, v| {
            if let Some(t) = app.tab_by_id(tid) {
                t.filter = v.unwrap_or(old2);
                t.reapply(None);
            }
        });
        if let Some(ModalKind::Prompt(p)) = self.modal_mut(id) {
            p.on_change = Some(Box::new(move |app: &mut App, text: &str| {
                if let Some(t) = app.tab_by_id(tid) {
                    t.filter = text.to_string();
                    t.reapply(None);
                }
            }));
        }
    }

    pub fn search_prompt(&mut self, forward: bool) {
        let title = if forward { "Chercher :" } else { "Chercher (en remontant) :" };
        let last = self.tab().search.clone();
        self.prompt(title, &last, None, move |app, v| {
            let Some(v) = v.filter(|s| !s.is_empty()) else { return };
            app.tab_mut().search = v.clone();
            app.find(&v, forward);
        });
    }

    pub fn recursive_search(&mut self, content: bool) {
        let title = if content { "Chercher dans le contenu des fichiers :" } else { "Chercher par nom (dans les sous-dossiers) :" };
        self.prompt(title, "", None, move |app, v| {
            let Some(pat) = v.filter(|s| !s.is_empty()) else { return };
            app.notify(&format!("Recherche de « {pat} »…"), Severity::Info, 2.0);
            let t = app.tab();
            let (fs, cwd, tid) = (t.fs.clone(), t.cwd.clone(), t.id);
            app.send_thread(move || {
                let r = crate::archive::find(&fs, &cwd, &pat, content);
                Box::new(move |app: &mut App| {
                    let (items, err) = match r {
                        Ok(v) => v,
                        Err(e) => (Vec::new(), e),
                    };
                    if items.is_empty() {
                        let mut msg = format!("Rien trouvé pour « {pat} ».");
                        if !err.trim().is_empty() {
                            msg.push('\n');
                            msg.push_str(err.trim());
                        }
                        app.notify(&msg, Severity::Warning, 5.0);
                        return;
                    }
                    let plus = if items.len() >= 2000 { "+" } else { "" };
                    let title = format!("{}{plus} résultat(s) pour « {pat} » dans {}:{}", items.len(), fs.label, fs.pretty(&cwd));
                    app.push_modal(ModalKind::Results(crate::modals::Results { title, items, cwd, tab: tid, cursor: 0, offset: 0 }));
                })
            });
        });
    }

    pub fn host_picker(&mut self) {
        self.push_modal(ModalKind::Hosts(crate::modals::Hosts::default()));
        if !self.discovered {
            self.start_discovery();
        }
    }

    pub fn goto_prompt(&mut self) {
        let t = self.tab();
        let mut init = t.fs.pretty(&t.cwd);
        if !init.ends_with('/') {
            init.push('/');
        }
        let id = self.prompt("Aller à  (Tab : compléter · machine:chemin pour une autre machine)", &init, None, |app, v| {
            let Some(v) = v.filter(|s| !s.trim().is_empty()) else { return };
            let (target, label, path, fs) = app.locate(v.trim());
            let Some(fs) = fs else {
                app.goto(target, label, path, None);
                return;
            };
            let p = path.unwrap_or_else(|| fs.home.clone());
            match fs.stat(&p) {
                Err(e) => app.error(&format!("{label}:{p} : {}", errstr(&e))),
                Ok(e) if e.is_dir => app.goto(target, label, Some(p), None),
                Ok(_) => {
                    let name = basename(&p).to_string();
                    app.goto(target, label, Some(dirname(&p)), Some(name));
                }
            }
        });
        if let Some(ModalKind::Prompt(p)) = self.modal_mut(id) {
            p.complete = Some(Box::new(|app: &App, text: &str| app.complete(text)));
        }
    }

    /// Complétion (Tab) d'un chemin « aller à ».
    pub fn complete(&self, text: &str) -> Option<(String, Vec<String>)> {
        let re = regex::Regex::new(r"^([^/~:][^/:]*):(.*)$").unwrap();
        let (prefix, rest) = match re.captures(text) {
            Some(c) => (format!("{}:", &c[1]), c[2].to_string()),
            None => (String::new(), text.to_string()),
        };
        let (_, _, _, fs) = self.locate(text);
        let fs = fs?;
        let (head, part) = match rest.rfind('/') {
            Some(i) => (rest[..=i].to_string(), rest[i + 1..].to_string()),
            None => (String::new(), rest.clone()),
        };
        let dir = if head.is_empty() {
            let t = self.tab();
            if same(&fs, &t.fs) { t.cwd.clone() } else { fs.home.clone() }
        } else {
            self.expand(&head, Some(&fs))?
        };
        let entries = fs.listdir(&dir).ok()?;
        let show_hidden = part.starts_with('.');
        let vis: Vec<_> = entries.iter().filter(|e| show_hidden || !e.name.starts_with('.')).collect();
        let mut cands: Vec<String> = vis.iter().filter(|e| e.name.starts_with(&part)).map(|e| fmt_cand(e)).collect();
        if cands.is_empty() {
            let lp = part.to_lowercase();
            cands = vis.iter().filter(|e| e.name.to_lowercase().starts_with(&lp)).map(|e| fmt_cand(e)).collect();
        }
        if cands.is_empty() {
            return Some((text.to_string(), Vec::new()));
        }
        let common = common_prefix(&cands);
        let best = if common.chars().count() >= part.chars().count() { common } else { part };
        let value = format!("{prefix}{head}{best}");
        cands.sort_by(|a, b| crate::util::natural_cmp(a, b));
        let list = if cands.len() > 1 { cands } else { Vec::new() };
        Some((value, list))
    }

    pub fn open_trash(&mut self) {
        let fs = self.tab().fs.clone();
        let id = self.push_modal(ModalKind::Trash(crate::modals::TrashM { fs: fs.clone(), items: None, cursor: 0, offset: 0 }));
        crate::modals::trash_reload(self, id, fs);
    }
}

fn fmt_cand(e: &crate::fs::Entry) -> String {
    if e.is_dir { format!("{}/", e.name) } else { e.name.clone() }
}

fn common_prefix(v: &[String]) -> String {
    let mut p: Vec<char> = v[0].chars().collect();
    for s in &v[1..] {
        let n = p.iter().zip(s.chars()).take_while(|(a, b)| **a == *b).count();
        p.truncate(n);
    }
    p.into_iter().collect()
}
