//! État de l'application, onglets, chargement des dossiers, clavier du navigateur.

use crate::bookmarks::{self, Bookmark, Bookmarks};
use crate::discover::{self, Host};
use crate::fs::{errstr, same, Entry, Fs, FsRef};
use crate::modals::{Modal, ModalKind};
use crate::preview::Preview;
use crate::transfers::{self, Shared};
use crate::util::{basename, dirname, join, natural_cmp, normpath};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub type Msg = Box<dyn FnOnce(&mut App) + Send>;

#[derive(Clone, Copy, PartialEq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

pub struct Toast {
    pub title: Option<String>,
    pub msg: String,
    pub sev: Severity,
    pub until: Instant,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SortKey {
    Natural,
    Mtime,
    Size,
}

impl SortKey {
    pub fn name(self) -> &'static str {
        match self {
            SortKey::Natural => "natural",
            SortKey::Mtime => "mtime",
            SortKey::Size => "size",
        }
    }
}

pub struct Tab {
    pub id: u64,
    pub fs: FsRef,
    pub cwd: String,
    pub raw: Option<Vec<Entry>>,
    pub entries: Vec<Entry>,
    pub parent: Vec<Entry>,
    pub parent_raw: Vec<Entry>,
    pub cursor: usize,
    pub offset: usize,
    pub parent_offset: usize,
    pub selected: BTreeMap<String, Entry>,
    pub history: HashMap<String, String>,
    pub hidden: bool,
    pub filter: String,
    pub search: String,
    pub sort: (SortKey, bool),
    pub visual: Option<usize>,
}

/// Tri et filtre d'une liste d'entrées.
pub fn arrange(raw: &[Entry], hidden: bool, filter: &str, sort: (SortKey, bool), keep: Option<&str>) -> Vec<Entry> {
    let f = filter.to_lowercase();
    let mut v: Vec<Entry> = raw
        .iter()
        .filter(|e| {
            Some(e.name.as_str()) == keep
                || ((hidden || !e.name.starts_with('.')) && (f.is_empty() || e.name.to_lowercase().contains(&f)))
        })
        .cloned()
        .collect();
    match sort.0 {
        SortKey::Natural => v.sort_by(|a, b| natural_cmp(&a.name, &b.name)),
        SortKey::Mtime => v.sort_by(|a, b| b.mtime.partial_cmp(&a.mtime).unwrap_or(std::cmp::Ordering::Equal)),
        SortKey::Size => v.sort_by(|a, b| b.size.cmp(&a.size)),
    }
    if sort.1 {
        v.reverse();
    }
    let (dirs, files): (Vec<Entry>, Vec<Entry>) = v.into_iter().partition(|e| e.is_dir);
    dirs.into_iter().chain(files).collect()
}

impl Tab {
    pub fn new(id: u64, fs: FsRef, cwd: String) -> Tab {
        Tab {
            id,
            fs,
            cwd,
            raw: None,
            entries: Vec::new(),
            parent: Vec::new(),
            parent_raw: Vec::new(),
            cursor: 0,
            offset: 0,
            parent_offset: 0,
            selected: BTreeMap::new(),
            history: HashMap::new(),
            hidden: false,
            filter: String::new(),
            search: String::new(),
            sort: (SortKey::Natural, false),
            visual: None,
        }
    }

    pub fn current(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }

    pub fn path_of(&self, name: &str) -> String {
        join(&self.cwd, name)
    }

    pub fn current_path(&self) -> Option<String> {
        self.current().map(|e| self.path_of(&e.name))
    }

    /// Recalcule la liste affichée, curseur sur `name` (sinon l'entrée actuelle).
    pub fn reapply(&mut self, name: Option<String>) {
        let name = name.or_else(|| self.current().map(|e| e.name.clone()));
        if let Some(raw) = &self.raw {
            self.entries = arrange(raw, self.hidden, &self.filter, self.sort, None);
        }
        self.place_cursor(name.as_deref());
        let keep = basename(&self.cwd).to_string();
        self.parent = arrange(&self.parent_raw, self.hidden, "", self.sort, Some(&keep));
    }

    fn place_cursor(&mut self, name: Option<&str>) {
        if let Some(i) = name.and_then(|n| self.entries.iter().position(|e| e.name == n)) {
            self.cursor = i;
        } else {
            self.cursor = self.cursor.min(self.entries.len().saturating_sub(1));
        }
    }

    pub fn visual_range(&self) -> Option<(usize, usize)> {
        self.visual.map(|a| (a.min(self.cursor), a.max(self.cursor)))
    }

    pub fn commit_visual(&mut self) {
        if let Some((a, b)) = self.visual_range() {
            for i in a..=b.min(self.entries.len().saturating_sub(1)) {
                if let Some(e) = self.entries.get(i) {
                    self.selected.insert(self.path_of(&e.name), e.clone());
                }
            }
        }
        self.visual = None;
    }

    /// Cibles d'une opération : plage visuelle validée, cochés, sinon l'entrée courante.
    pub fn targets(&mut self) -> Vec<(String, Entry)> {
        self.commit_visual();
        if !self.selected.is_empty() {
            return self.selected.iter().map(|(p, e)| (p.clone(), e.clone())).collect();
        }
        self.current().map(|e| vec![(self.path_of(&e.name), e.clone())]).unwrap_or_default()
    }

    pub fn parent_dir(&self) -> Option<String> {
        if self.cwd == "/" { None } else { Some(dirname(&self.cwd)) }
    }
}

pub struct Yank {
    pub fs: FsRef,
    pub items: BTreeMap<String, Entry>,
    pub cut: bool,
}

pub type PreviewKey = (usize, String, u64, u64);

pub struct PreviewState {
    pub cache: HashMap<PreviewKey, Arc<Preview>>,
    pub current: Option<(PreviewKey, Arc<Preview>)>,
    pub want: Option<PreviewKey>,
    pub due: Option<Instant>,
    pub generation: u64,
    pub du: HashMap<(usize, String, u64), u64>,
    pub du_pending: Option<(usize, String, u64)>,
    pub du_cancel: Arc<AtomicBool>,
    pub image: Option<(String, StatefulProtocol)>,
}

#[derive(Default, Clone, Copy)]
pub struct Areas {
    pub parent: Rect,
    pub current: Rect,
    pub preview: Rect,
}

pub struct App {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub local: FsRef,
    pub remotes: HashMap<String, FsRef>,
    pub yank: Option<Yank>,
    pub pending: Option<char>,
    pub modals: Vec<Modal>,
    pub toasts: Vec<Toast>,
    pub hosts: Vec<Host>,
    pub discovering: bool,
    pub discovered: bool,
    pub bookmarks: Bookmarks,
    pub transfers: Shared,
    pub tx: Sender<Msg>,
    pub preview: PreviewState,
    pub list_gen: u64,
    pub quit: bool,
    pub needs_clear: bool,
    pub areas: Areas,
    pub last_click: Option<(Instant, u16, u16)>,
    pub picker: Picker,
    pub tick: u64,
    pub next_id: u64,
    pub col_height: usize,
    /// Dernier emplacement refusé (pour proposer sudo / autre utilisateur).
    pub last_denied: Option<(FsRef, String)>,
    /// Opérations annulables (Ctrl+Z).
    pub history: crate::undo::History,
}

impl App {
    pub fn new(tx: Sender<Msg>, picker: Picker) -> App {
        let local: FsRef = Arc::new(Fs::local());
        App {
            tabs: Vec::new(),
            active: 0,
            local,
            remotes: HashMap::new(),
            yank: None,
            pending: None,
            modals: Vec::new(),
            toasts: Vec::new(),
            hosts: Vec::new(),
            discovering: false,
            discovered: false,
            bookmarks: bookmarks::load(),
            transfers: transfers::new_shared(),
            tx,
            preview: PreviewState {
                cache: HashMap::new(),
                current: None,
                want: None,
                due: None,
                generation: 0,
                du: HashMap::new(),
                du_pending: None,
                du_cancel: Arc::new(AtomicBool::new(false)),
                image: None,
            },
            list_gen: 0,
            quit: false,
            needs_clear: false,
            areas: Areas::default(),
            last_click: None,
            picker,
            tick: 0,
            next_id: 1,
            col_height: 20,
            last_denied: None,
            history: Default::default(),
        }
    }

    pub fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    pub fn tab_by_id(&mut self, id: u64) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn notify(&mut self, msg: &str, sev: Severity, secs: f64) {
        log_sev(sev, msg);
        self.toasts.push(Toast { title: None, msg: msg.to_string(), sev, until: Instant::now() + Duration::from_secs_f64(secs) });
        if sev == Severity::Error && crate::auth::is_denied(msg) {
            self.maybe_offer_elevation();
        }
    }

    pub fn notify_t(&mut self, msg: &str, title: &str, sev: Severity, secs: f64) {
        log_sev(sev, &format!("{title} : {msg}"));
        self.toasts.push(Toast {
            title: Some(title.to_string()),
            msg: msg.to_string(),
            sev,
            until: Instant::now() + Duration::from_secs_f64(secs),
        });
        if sev == Severity::Error && crate::auth::is_denied(msg) {
            self.maybe_offer_elevation();
        }
    }

    pub fn info(&mut self, msg: &str) {
        self.notify(msg, Severity::Info, 5.0);
    }

    pub fn warn(&mut self, msg: &str) {
        self.notify(msg, Severity::Warning, 5.0);
    }

    pub fn error(&mut self, msg: &str) {
        self.notify(msg, Severity::Error, 5.0);
    }

    pub fn push_modal(&mut self, kind: ModalKind) -> u64 {
        let id = self.new_id();
        self.modals.push(Modal { id, kind });
        id
    }

    pub fn modal_mut(&mut self, id: u64) -> Option<&mut ModalKind> {
        self.modals.iter_mut().find(|m| m.id == id).map(|m| &mut m.kind)
    }

    pub fn has_transfers(&self) -> bool {
        !self.transfers.0.lock().unwrap().tasks.is_empty()
    }

    // ------------------------------------------------------------ FS, onglets

    pub fn fs_for(&self, target: Option<&str>) -> Option<FsRef> {
        match target {
            None => Some(self.local.clone()),
            Some(t) => self.remotes.get(t).cloned(),
        }
    }

    pub fn add_tab(&mut self, fs: FsRef, path: String, hover: Option<String>) {
        crate::log::info(&format!("onglet ouvert : {}:{}", fs.label, fs.pretty(&path)));
        let id = self.new_id();
        self.tabs.push(Tab::new(id, fs, path.clone()));
        self.active = self.tabs.len() - 1;
        self.load(path, hover);
    }

    pub fn open_tab(&mut self, target: Option<String>, label: String, path: Option<String>, hover: Option<String>) {
        let Some(target) = target else {
            let p = path.unwrap_or_else(|| self.local.home.clone());
            self.add_tab(self.local.clone(), p, hover);
            return;
        };
        if let Some(fs) = self.remotes.get(&target).cloned() {
            let p = path.unwrap_or_else(|| fs.home.clone());
            self.add_tab(fs, p, hover);
            return;
        }
        self.notify(&format!("Connexion à {label} ({target})…"), Severity::Info, 3.0);
        connect_thread(self.tx.clone(), target, label, path, hover, true);
    }

    pub fn goto(&mut self, target: Option<String>, label: String, path: Option<String>, hover: Option<String>) {
        let Some(fs) = self.fs_for(target.as_deref()) else {
            self.open_tab(target, label, path, hover);
            return;
        };
        if !same(&self.tab().fs, &fs) {
            if let Some(i) = self.tabs.iter().position(|t| same(&t.fs, &fs)) {
                self.active = i;
            } else {
                let p = path.clone().unwrap_or_else(|| fs.home.clone());
                self.add_tab(fs, p, hover);
                return;
            }
        }
        let p = path.unwrap_or_else(|| fs.home.clone());
        self.load(p, hover);
    }

    pub fn close_tab(&mut self) {
        if self.tabs.len() <= 1 {
            self.request_quit();
            return;
        }
        let t = self.tabs.remove(self.active);
        crate::log::info(&format!("onglet fermé : {}:{}", t.fs.label, t.fs.pretty(&t.cwd)));
        self.active = self.active.min(self.tabs.len() - 1);
        let cwd = self.tab().cwd.clone();
        self.load(cwd, None);
    }

    pub fn switch_tab(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.active = i;
            let cwd = self.tab().cwd.clone();
            self.load(cwd, None);
        }
    }

    /// Met un transfert en file ; à la fin : notification et rafraîchissement.
    pub fn enqueue(&self, task: transfers::Task) {
        let tx = self.tx.clone();
        transfers::enqueue(&self.transfers, task, move |d| {
            let _ = tx.send(Box::new(move |app: &mut App| {
                crate::notify::transfer_done(&d);
                match d.result {
                    Ok(()) => app.notify_t(&d.title, "Terminé", Severity::Info, 4.0),
                    Err(msg) => app.notify_t(&msg, &d.title, Severity::Error, 10.0),
                }
                app.history.push(crate::undo::UndoOp::Batch(d.undo));
                app.refresh_fs(&d.touches);
            }));
        });
    }

    /// Ctrl+Z : annule la dernière opération (renommage, déplacement, corbeille, création, copie).
    pub fn undo_last(&mut self) {
        let Some(op) = self.history.pop() else {
            self.info("Rien à annuler.");
            return;
        };
        let what = op.describe();
        self.send_thread(move || {
            let r = op.undo();
            Box::new(move |app: &mut App| match r {
                Ok(touched) => {
                    app.info(&format!("Annulé : {what}"));
                    app.refresh_fs(&touched);
                }
                Err(e) => app.error(&format!("Annulation impossible : {e}")),
            })
        });
    }

    pub fn request_quit(&mut self) {
        if self.has_transfers() {
            self.confirm("Des transferts sont en cours. Quitter quand même ?", |app| app.quit = true);
        } else {
            self.quit = true;
        }
    }

    /// « : » : ligne de commande en bas de l'écran, validée par Entrée (comme helix).
    pub fn command_line(&mut self) {
        let id = self.prompt(":", "", None, |app, v| {
            let Some(v) = v else { return };
            if !v.trim().is_empty() {
                crate::log::info(&format!("commande :{}", v.trim()));
            }
            match v.trim() {
                "" => {}
                "q" | "quit" | "qa" | "qall" | "quit-all" => app.request_quit(),
                "q!" | "quit!" | "qa!" | "qall!" | "quit-all!" => app.quit = true,
                "gui" | "gtk" => app.to_gui(),
                "airdrop" => app.airdrop(),
                // comme `!` : shell plein écran dans le dossier courant
                "shell" | "sh" | "terminal" | "term" | "!" => app.shell(),
                "rsync" => {
                    let rs = crate::config::rsync_status();
                    let sev = if rs.usable() { Severity::Info } else { Severity::Warning };
                    app.notify(&rs.summary(), sev, 8.0);
                }
                "help" | "h" => {
                    app.push_modal(ModalKind::Help(Default::default()));
                }
                c => app.notify(&format!("Commande inconnue : {c}  (:help pour la liste)"), Severity::Error, 3.0),
            }
        });
        if let Some(ModalKind::Prompt(p)) = self.modal_mut(id) {
            p.cmdline = true;
        }
    }

    // ------------------------------------------------------------ chargement

    pub fn load(&mut self, path: String, hover: Option<String>) {
        if self.tabs.is_empty() {
            return;
        }
        self.list_gen += 1;
        let generation = self.list_gen;
        let tab = self.tab();
        let (tid, fs) = (tab.id, tab.fs.clone());
        if let Some(c) = fs.cached(&path) {
            self.apply_listing(tid, &path, c, hover.clone());
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let res = fs.ls(&path);
            let parent = if path == "/" {
                Vec::new()
            } else {
                let p = dirname(&path);
                fs.cached(&p).or_else(|| fs.ls(&p).ok()).unwrap_or_default()
            };
            let _ = tx.send(Box::new(move |app: &mut App| {
                if app.list_gen != generation {
                    return;
                }
                match res {
                    Ok(v) => {
                        app.apply_listing(tid, &path, v, hover);
                        if let Some(t) = app.tab_by_id(tid) {
                            if t.cwd == path {
                                t.parent_raw = parent;
                                t.reapply(None);
                            }
                        }
                    }
                    Err(e) => {
                        let label = fs.label.clone();
                        app.last_denied = Some((fs.clone(), path.clone()));
                        app.notify_t(&format!("{path} : {}", errstr(&e)), &label, Severity::Error, 5.0);
                        let never = app.tab_by_id(tid).map(|t| t.raw.is_none()).unwrap_or(false);
                        if never && path != "/" {
                            let idx = app.tabs.iter().position(|t| t.id == tid);
                            if let Some(i) = idx {
                                if i == app.active {
                                    app.load(dirname(&path), None);
                                }
                            }
                        }
                    }
                }
            }));
        });
    }

    fn apply_listing(&mut self, tid: u64, path: &str, raw: Vec<Entry>, hover: Option<String>) {
        let Some(t) = self.tab_by_id(tid) else { return };
        let changed = t.cwd != path || t.raw.is_none();
        if t.cwd != path {
            crate::log::info(&format!("dossier {}:{}", t.fs.label, t.fs.pretty(path)));
        }
        let name = hover.or_else(|| if t.cwd == path { t.current().map(|e| e.name.clone()) } else { None }).or_else(|| t.history.get(path).cloned());
        if t.cwd != path {
            if let Some(cur) = t.current() {
                let n = cur.name.clone();
                let c = t.cwd.clone();
                t.history.insert(c, n);
            }
            t.visual = None;
            t.filter.clear();
            t.parent_raw.clear();
            t.parent.clear();
            t.offset = 0;
            t.parent_offset = 0;
        }
        let _ = changed;
        t.cwd = path.to_string();
        t.raw = Some(raw);
        t.reapply(name);
        if let Some(cur) = t.current() {
            let n = cur.name.clone();
            t.history.insert(path.to_string(), n);
        }
    }

    pub fn cd(&mut self, path: String, hover: Option<String>) {
        self.load(path, hover);
    }

    pub fn reload(&mut self, hover: Option<String>) {
        let cwd = self.tab().cwd.clone();
        self.load(cwd, hover);
    }

    pub fn refresh_fs(&mut self, list: &[FsRef]) {
        for fs in list {
            fs.clear_cache();
        }
        self.preview.cache.clear();
        self.preview.current = None;
        self.preview.want = None;
        self.preview.du.clear();
        if !self.tabs.is_empty() && list.iter().any(|f| same(f, &self.tab().fs)) {
            self.reload(None);
        }
    }

    /// Rafraîchissement automatique (toutes les 2 s).
    pub fn auto_refresh(&mut self) {
        self.tick += 1;
        if !self.modals.is_empty() || self.tabs.is_empty() || self.tab().raw.is_none() {
            return;
        }
        let tab = self.tab();
        if tab.fs.remote && self.tick % 3 != 0 {
            return;
        }
        let (tid, fs, path) = (tab.id, tab.fs.clone(), tab.cwd.clone());
        self.send_thread(move || {
            let res = fs.ls(&path);
            Box::new(move |app: &mut App| {
                let Ok(v) = res else { return };
                let Some(t) = app.tab_by_id(tid) else { return };
                if t.cwd != path {
                    return;
                }
                let sig = |l: &[Entry]| {
                    let mut s: Vec<(String, u64, u64, bool)> = l.iter().map(|e| (e.name.clone(), e.size, e.mtime.to_bits(), e.is_dir)).collect();
                    s.sort();
                    s
                };
                if t.raw.as_deref().map(sig) != Some(sig(&v)) {
                    t.raw = Some(v);
                    t.reapply(None);
                    app.preview.want = None;
                }
            })
        });
    }

    /// Lance `f` dans un thread ; son résultat (une fermeture) est appliqué sur le fil principal.
    pub fn send_thread<F>(&self, f: F)
    where
        F: FnOnce() -> Msg + Send + 'static,
    {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let m = f();
            let _ = tx.send(m);
        });
    }

    pub fn start_discovery(&mut self) {
        if self.discovering {
            return;
        }
        self.discovering = true;
        self.send_thread(|| {
            let hosts = discover::discover();
            Box::new(move |app: &mut App| {
                app.hosts = hosts;
                app.discovering = false;
                app.discovered = true;
            })
        });
    }

    // ------------------------------------------------------------ boîtes de dialogue

    pub fn confirm(&mut self, msg: &str, on_yes: impl FnOnce(&mut App) + 'static) {
        self.confirm_full(msg, None, Box::new(on_yes), None);
    }

    pub fn confirm_full(
        &mut self,
        msg: &str,
        extra: Option<String>,
        on_yes: Box<dyn FnOnce(&mut App)>,
        on_no: Option<Box<dyn FnOnce(&mut App)>>,
    ) {
        self.push_modal(ModalKind::Confirm(crate::modals::Confirm { msg: msg.to_string(), extra, on_yes: Some(on_yes), on_no }));
    }

    pub fn prompt(&mut self, title: &str, value: &str, cursor: Option<usize>, on_submit: impl FnOnce(&mut App, Option<String>) + 'static) -> u64 {
        self.push_modal(ModalKind::Prompt(crate::modals::Prompt::new(title, value, cursor, Box::new(on_submit))))
    }

    // ------------------------------------------------------------ déplacements

    pub fn move_cursor(&mut self, delta: isize) {
        let t = self.tab_mut();
        let n = t.entries.len();
        if n == 0 {
            return;
        }
        let c = (t.cursor as isize + delta).clamp(0, n as isize - 1);
        t.cursor = c as usize;
    }

    pub fn set_cursor(&mut self, i: usize) {
        let t = self.tab_mut();
        if !t.entries.is_empty() {
            t.cursor = i.min(t.entries.len() - 1);
        }
    }

    pub fn go_parent(&mut self) {
        let t = self.tab();
        if let Some(p) = t.parent_dir() {
            let name = basename(&t.cwd).to_string();
            self.cd(p, Some(name));
        }
    }

    pub fn enter(&mut self) {
        let t = self.tab();
        let Some(e) = t.current().cloned() else { return };
        let path = t.path_of(&e.name);
        if e.is_dir {
            self.cd(path, None);
        } else {
            self.open_file(path);
        }
    }

    // ------------------------------------------------------------ clavier

    pub fn on_key(&mut self, k: KeyEvent) {
        let Some(name) = key_name(&k) else { return };
        if !self.modals.is_empty() {
            let idx = self.modals.len() - 1;
            let mut m = self.modals.remove(idx);
            let keep = crate::modals::handle_key(self, &mut m, &k, &name);
            if keep {
                let pos = idx.min(self.modals.len());
                self.modals.insert(pos, m);
            }
            return;
        }
        if self.tabs.is_empty() {
            return;
        }
        if let Some(p) = self.pending.take() {
            if name != "escape" {
                self.combo(p, &name);
            }
            return;
        }
        self.key(&name);
    }

    fn key(&mut self, name: &str) {
        let half = (self.col_height / 2).max(1) as isize;
        let page = self.col_height.max(1) as isize;
        match name {
            ":" => self.command_line(),
            "g" | "c" | "," | "m" | "=" | "-" => self.pending = name.chars().next(),
            // shell plein écran dans le dossier courant (vrai terminal, ssh à distance)
            "!" => self.shell(),
            "j" | "down" => self.move_cursor(1),
            "k" | "up" => self.move_cursor(-1),
            "ctrl+d" => self.move_cursor(half),
            "ctrl+u" => self.move_cursor(-half),
            "pagedown" => self.move_cursor(page),
            "pageup" => self.move_cursor(-page),
            "G" | "end" => {
                let n = self.tab().entries.len();
                self.set_cursor(n.saturating_sub(1));
            }
            "home" => self.set_cursor(0),
            "h" | "left" => self.go_parent(),
            "l" | "right" | "enter" => self.enter(),
            "o" => {
                if let Some(p) = self.tab().current_path() {
                    let is_dir = self.tab().current().map(|e| e.is_dir).unwrap_or(false);
                    self.xdg_open(p, is_dir);
                }
            }
            "space" => {
                let t = self.tab_mut();
                if let Some(e) = t.current().cloned() {
                    let p = t.path_of(&e.name);
                    if t.selected.remove(&p).is_none() {
                        t.selected.insert(p, e);
                    }
                    self.move_cursor(1);
                }
            }
            "v" => {
                let t = self.tab_mut();
                if t.visual.is_some() {
                    t.commit_visual();
                } else if !t.entries.is_empty() {
                    t.visual = Some(t.cursor);
                }
            }
            "ctrl+a" => {
                let t = self.tab_mut();
                let all: Vec<(String, Entry)> = t.entries.iter().map(|e| (t.path_of(&e.name), e.clone())).collect();
                t.selected.extend(all);
            }
            "ctrl+r" => {
                let t = self.tab_mut();
                let all: Vec<(String, Entry)> = t.entries.iter().map(|e| (t.path_of(&e.name), e.clone())).collect();
                for (p, e) in all {
                    if t.selected.remove(&p).is_none() {
                        t.selected.insert(p, e);
                    }
                }
            }
            "escape" => {
                let t = self.tab_mut();
                if t.visual.is_some() {
                    t.commit_visual();
                } else if !t.filter.is_empty() {
                    t.filter.clear();
                    t.reapply(None);
                } else {
                    t.selected.clear();
                }
            }
            "y" | "x" => self.yank(name == "x"),
            "Y" | "X" => self.yank = None,
            "p" => self.paste(false),
            "P" => self.paste(true),
            "d" => self.delete(false),
            "D" => self.delete(true),
            "a" => self.create(),
            "r" => self.rename(),
            "." => {
                let t = self.tab_mut();
                t.hidden = !t.hidden;
                t.reapply(None);
            }
            "C" => self.compress(),
            "E" => self.extract(),
            "T" => self.open_trash(),
            "U" => {
                let (fs, cwd) = (self.tab().fs.clone(), self.tab().cwd.clone());
                self.offer_elevation(fs, cwd);
            }
            "f" => self.filter_prompt(),
            "/" => self.search_prompt(true),
            "?" => self.search_prompt(false),
            "n" => {
                let s = self.tab().search.clone();
                if !s.is_empty() {
                    self.find(&s, true);
                }
            }
            "N" => {
                let s = self.tab().search.clone();
                if !s.is_empty() {
                    self.find(&s, false);
                }
            }
            "s" => self.recursive_search(false),
            "S" => self.recursive_search(true),
            "t" => self.host_picker(),
            "ctrl+c" => self.close_tab(),
            "ctrl+g" => self.to_gui(),
            "ctrl+z" => self.undo_last(),
            "A" => self.airdrop(),
            "[" | "ctrl+shift+tab" => {
                let n = self.tabs.len();
                self.switch_tab((self.active + n - 1) % n);
            }
            "]" | "ctrl+tab" => {
                let n = self.tabs.len();
                self.switch_tab((self.active + 1) % n);
            }
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => {
                let i = name.parse::<usize>().unwrap() - 1;
                self.switch_tab(i);
            }
            "w" => {
                self.push_modal(ModalKind::Transfers);
            }
            "R" => {
                let fs = self.tab().fs.clone();
                self.refresh_fs(&[fs]);
            }
            "'" => self.open_bookmarks(),
            "ctrl+h" | "backspace" | "~" | "f1" => {
                self.push_modal(ModalKind::Help(Default::default()));
            }
            _ => {}
        }
    }

    fn combo(&mut self, prefix: char, name: &str) {
        match prefix {
            'g' => {
                let home = self.tab().fs.home.clone();
                match name {
                    "g" => self.set_cursor(0),
                    "space" => self.goto_prompt(),
                    "h" => self.cd(home, None),
                    "c" => self.cd(join(&home, ".config"), None),
                    "d" => self.cd(join(&home, "Downloads"), None),
                    "/" => self.cd("/".into(), None),
                    _ => {}
                }
            }
            'c' => {
                let t = self.tab();
                let Some(e) = t.current() else { return };
                let text = match name {
                    "c" => t.fs.spec(&t.path_of(&e.name)),
                    "f" => e.name.clone(),
                    "d" => t.fs.spec(&t.cwd),
                    _ => return,
                };
                osc52(&text);
                self.notify(&format!("Copié : {text}"), Severity::Info, 3.0);
            }
            ',' => {
                let (key, rev) = match name {
                    "n" => (SortKey::Natural, false),
                    "m" => (SortKey::Mtime, false),
                    "s" => (SortKey::Size, false),
                    "N" => (SortKey::Natural, true),
                    "M" => (SortKey::Mtime, true),
                    "S" => (SortKey::Size, true),
                    _ => return,
                };
                let t = self.tab_mut();
                t.sort = (key, rev);
                t.reapply(None);
            }
            'm' => {
                let mut cs = name.chars();
                let (Some(c), None) = (cs.next(), cs.next()) else { return };
                if !c.is_alphanumeric() {
                    return;
                }
                let t = self.tab();
                let bm = Bookmark {
                    target: if t.fs.remote { Some(t.fs.target.clone()) } else { None },
                    label: t.fs.label.clone(),
                    path: t.cwd.clone(),
                };
                let msg = format!("Marque-page **{c}** → {}:{}\n' puis {c} pour y revenir.", t.fs.label, t.fs.pretty(&t.cwd));
                self.bookmarks.insert(c.to_string(), bm);
                bookmarks::save(&self.bookmarks);
                self.notify(&msg, Severity::Info, 4.0);
            }
            '=' | '-' => {
                let Some(n) = name.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) else {
                    return;
                };
                if prefix == '=' {
                    self.compare_dirs(n - 1);
                } else {
                    self.compare_files(n - 1);
                }
            }
            _ => {}
        }
    }

    pub fn find(&mut self, pat: &str, forward: bool) {
        let t = self.tab();
        let n = t.entries.len();
        let p = pat.to_lowercase();
        for k in 1..=n {
            let i = if forward { (t.cursor + k) % n } else { (t.cursor + n * 2 - k) % n };
            if t.entries[i].name.to_lowercase().contains(&p) {
                self.set_cursor(i);
                return;
            }
        }
        self.notify(&format!("Introuvable : {pat}"), Severity::Warning, 3.0);
    }

    // ------------------------------------------------------------ souris

    pub fn on_mouse(&mut self, m: MouseEvent) {
        if let Some(crate::modals::Modal { kind: crate::modals::ModalKind::Term(t), .. }) = self.modals.last_mut() {
            if let Some(text) = t.mouse(&m) {
                osc52(&text);
                let n = text.chars().count();
                self.notify(&format!("Copié ({n} caractères)."), Severity::Info, 2.0);
            }
            return;
        }
        if !self.modals.is_empty() || self.tabs.is_empty() {
            return;
        }
        let inside = |r: Rect| m.column >= r.x && m.column < r.x + r.width && m.row >= r.y && m.row < r.y + r.height;
        let cur = self.areas.current;
        let par = self.areas.parent;
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) if inside(cur) => {
                let idx = self.tab().offset + (m.row - cur.y) as usize;
                if idx >= self.tab().entries.len() {
                    return;
                }
                let now = Instant::now();
                let double = matches!(self.last_click, Some((t, _, r)) if now.duration_since(t) < Duration::from_millis(400) && r == m.row);
                self.set_cursor(idx);
                if double {
                    self.last_click = None;
                    self.enter();
                } else {
                    self.last_click = Some((now, m.column, m.row));
                }
            }
            MouseEventKind::Down(MouseButton::Left) if inside(par) => {
                let t = self.tab();
                let idx = t.parent_offset + (m.row - par.y) as usize;
                if let (Some(e), Some(pd)) = (t.parent.get(idx), t.parent_dir()) {
                    if e.is_dir {
                        let p = join(&pd, &e.name);
                        self.cd(p, None);
                    }
                }
            }
            MouseEventKind::ScrollDown if inside(cur) => self.move_cursor(3),
            MouseEventKind::ScrollUp if inside(cur) => self.move_cursor(-3),
            _ => {}
        }
    }

    // ------------------------------------------------------------ aperçu

    pub fn hover_key(&self) -> Option<PreviewKey> {
        let t = self.tabs.get(self.active)?;
        let e = t.current()?;
        Some((Arc::as_ptr(&t.fs) as usize, t.path_of(&e.name), e.mtime.to_bits(), e.size))
    }

    /// Appelé à chaque tour de boucle : planifie / lance le calcul de l'aperçu.
    pub fn update_preview(&mut self, height: usize) {
        let key = self.hover_key();
        if key != self.preview.want {
            self.preview.want = key.clone();
            self.preview.due = Some(Instant::now() + Duration::from_millis(50));
            self.preview.du_cancel.store(true, Ordering::Relaxed);
            if key.is_none() {
                self.preview.current = None;
            }
        }
        let Some(due) = self.preview.due else { return };
        if Instant::now() < due {
            return;
        }
        self.preview.due = None;
        let Some(key) = key else { return };
        if let Some(p) = self.preview.cache.get(&key).cloned() {
            self.preview.current = Some((key.clone(), p));
            self.start_du(&key);
            return;
        }
        self.preview.generation += 1;
        let generation = self.preview.generation;
        let t = self.tab();
        let fs = t.fs.clone();
        let e = t.current().cloned().unwrap();
        self.send_thread(move || {
            let p = crate::preview::compute(&fs, &key.1, &e, height);
            Box::new(move |app: &mut App| {
                if app.preview.generation != generation || app.preview.want.as_ref() != Some(&key) {
                    return;
                }
                let p = Arc::new(p);
                if !matches!(&*p, Preview::Error(_)) {
                    app.preview.cache.insert(key.clone(), p.clone());
                }
                app.preview.current = Some((key.clone(), p));
                app.start_du(&key);
            })
        });
    }

    fn start_du(&mut self, key: &PreviewKey) {
        let Some((_, p)) = &self.preview.current else { return };
        if !matches!(&**p, Preview::Dir { .. }) {
            return;
        }
        let dk = (key.0, key.1.clone(), key.2);
        if self.preview.du.contains_key(&dk) || self.preview.du_pending.as_ref() == Some(&dk) {
            return;
        }
        self.preview.du_pending = Some(dk.clone());
        let cancel = Arc::new(AtomicBool::new(false));
        self.preview.du_cancel = cancel.clone();
        let fs = self.tab().fs.clone();
        self.send_thread(move || {
            let argv = fs.sh_argv("du -sk -- \"$1\"", &[&dk.1]);
            let r = crate::proc::run_full(&argv, fs.secret(), &[], Some(Duration::from_secs(30)), Some(&cancel));
            Box::new(move |app: &mut App| {
                if app.preview.du_pending.as_ref() == Some(&dk) {
                    app.preview.du_pending = None;
                }
                if let Ok(o) = r {
                    if let Some(k) = o.stdout.split_whitespace().next().and_then(|s| s.parse::<u64>().ok()) {
                        app.preview.du.insert(dk, k * 1024);
                    }
                }
            })
        });
    }

    pub fn expire_toasts(&mut self) {
        let now = Instant::now();
        self.toasts.retain(|t| t.until > now);
    }

    // ------------------------------------------------------------ marque-pages

    pub fn open_bookmarks(&mut self) {
        self.push_modal(ModalKind::Bookmarks(0));
    }

    pub fn goto_bookmark(&mut self, key: &str) {
        if let Some(b) = self.bookmarks.get(key).cloned() {
            self.goto(b.target, b.label, Some(b.path), None);
        }
    }

    // ------------------------------------------------------------ « aller à »

    /// `nom:chemin` → (target, label, chemin développé ou None, FS connu).
    pub fn locate(&self, text: &str) -> (Option<String>, String, Option<String>, Option<FsRef>) {
        let re = regex::Regex::new(r"^([^/~:][^/:]*):(.*)$").unwrap();
        let tab = self.tab();
        let (target, label, rest, fs): (Option<String>, String, String, Option<FsRef>) = match re.captures(text) {
            None => (
                if tab.fs.remote { Some(tab.fs.target.clone()) } else { None },
                tab.fs.label.clone(),
                text.to_string(),
                Some(tab.fs.clone()),
            ),
            Some(c) => {
                let name = c[1].to_string();
                let rest = c[2].to_string();
                if name == "local" {
                    (None, "local".into(), rest, Some(self.local.clone()))
                } else if let Some(fs) = self.remotes.values().find(|f| f.label == name || f.target == name) {
                    (Some(fs.target.clone()), fs.label.clone(), rest, Some(fs.clone()))
                } else if let Some(h) = self.hosts.iter().find(|h| h.names.contains(&name) || h.target == name) {
                    (Some(h.target.clone()), name.clone(), rest, None)
                } else {
                    (Some(name.clone()), name, rest, None)
                }
            }
        };
        let path = self.expand(&rest, fs.as_ref());
        (target, label, path, fs)
    }

    pub fn expand(&self, rest: &str, fs: Option<&FsRef>) -> Option<String> {
        let tab = self.tab();
        match fs {
            Some(fs) => {
                if rest.is_empty() {
                    return Some(fs.home.clone());
                }
                let p = if rest == "~" {
                    fs.home.clone()
                } else if let Some(r) = rest.strip_prefix("~/") {
                    join(&fs.home, r)
                } else if rest.starts_with('/') {
                    rest.to_string()
                } else {
                    let base = if same(fs, &tab.fs) { tab.cwd.clone() } else { fs.home.clone() };
                    join(&base, rest)
                };
                Some(normpath(&p))
            }
            None => {
                if rest.starts_with('/') { Some(normpath(rest)) } else { None }
            }
        }
    }
}

/// Connexion à une machine distante dans un thread.
fn connect_thread(tx: Sender<Msg>, target: String, label: String, path: Option<String>, hover: Option<String>, allow_interactive: bool) {
    std::thread::spawn(move || {
        // test de connexion (rouvre une connexion maîtresse morte au besoin)
        let r = crate::auth::check_ssh(&target);
        let (ok, stderr) = match &r {
            Ok(o) => (o.ok(), o.stderr.trim().to_string()),
            Err(e) => (false, e.to_string()),
        };
        if !ok {
            let interactive = ["Host key verification failed", "Permission denied", "passphrase"].iter().any(|s| stderr.contains(s));
            let _ = tx.send(Box::new(move |app: &mut App| {
                if interactive && allow_interactive {
                    crate::log::info(&format!("{target} : connexion interactive ({stderr})"));
                    crate::tui::suspend(app, || {
                        println!("\nmySend — connexion interactive à {target}\n");
                        let mut c = std::process::Command::new("ssh");
                        c.args(crate::config::ssh_opts()).arg(&target).arg("true");
                        let _ = c.status();
                    });
                    connect_thread(app.tx.clone(), target, label, path, hover, false);
                } else {
                    let msg = if stderr.is_empty() { "Connexion impossible.".to_string() } else { stderr };
                    app.notify_t(&msg, &label, Severity::Error, 8.0);
                }
            }));
            return;
        }
        let res = Fs::connect_remote(&target, &label);
        let _ = tx.send(Box::new(move |app: &mut App| match res {
            Ok(fs) => {
                crate::log::info(&format!("connecté à {label} ({target})"));
                let fs = app.remotes.entry(target.clone()).or_insert_with(|| Arc::new(fs)).clone();
                let p = path.unwrap_or_else(|| fs.home.clone());
                app.add_tab(fs, p, hover);
            }
            Err(e) => app.notify_t(&e.to_string(), &label, Severity::Error, 8.0),
        }));
    });
}

/// Recopie une notification dans mysend.log.
fn log_sev(sev: Severity, msg: &str) {
    match sev {
        Severity::Error => crate::log::error(msg),
        Severity::Warning => crate::log::warn(msg),
        _ => crate::log::info(msg),
    }
}

/// Nom de touche : le caractère s'il est imprimable (≠ espace), sinon le nom.
pub fn key_name(k: &KeyEvent) -> Option<String> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let base = match k.code {
        KeyCode::Modifier(_) | KeyCode::CapsLock | KeyCode::NumLock | KeyCode::ScrollLock => return None,
        KeyCode::Char(c) => {
            if ctrl {
                return Some(format!("ctrl+{}", c.to_lowercase()));
            }
            if alt {
                return Some(format!("alt+{c}"));
            }
            if c == ' ' {
                return Some("space".into());
            }
            return Some(c.to_string());
        }
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Esc => "escape".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::BackTab => return Some(if ctrl { "ctrl+shift+tab" } else { "shift+tab" }.into()),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::PageUp => "pageup".into(),
        KeyCode::PageDown => "pagedown".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::Insert => "insert".into(),
        KeyCode::F(n) => format!("f{n}"),
        _ => return None,
    };
    Some(if ctrl { format!("ctrl+{base}") } else { base })
}

/// Copie dans le presse-papiers du terminal (OSC 52).
pub fn osc52(text: &str) {
    use std::io::Write;
    let b64 = base64(text.as_bytes());
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{b64}\x07");
    let _ = out.flush();
}

fn base64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for ch in data.chunks(3) {
        let b = [ch[0], *ch.get(1).unwrap_or(&0), *ch.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= ch.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}
