//! Fenêtre principale : barre latérale (machines, marque-pages), onglets, aperçu, transferts.

use crate::dialogs::{self, Field};
use crate::pane::{DragItems, Pane};
use crate::preview::Preview;
use crate::transfers_view::TransfersView;
use crate::util::{bg, in_terminal, on_main};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use mysend::auth::{bare, is_denied, ssh_with_password};
use mysend::bookmarks::{self, Bookmark, Bookmarks};
use mysend::config::mkdtemp;
use mysend::discover::{self, Host};
use mysend::fs::{errstr, same, Entry, Fs, FsRef};
use mysend::session::{connect_all, sibling, Session};
use mysend::transfers::{self, Done, Shared};
use mysend::util::{basename, dirname, join, normpath};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

pub struct Clip {
    pub fs: FsRef,
    pub items: Vec<(String, Entry)>,
    pub cut: bool,
}

enum SideItem {
    Host(Option<String>, String),
    Bookmark(String),
}

pub struct Win {
    pub app: adw::Application,
    pub window: adw::ApplicationWindow,
    pub(crate) toasts: adw::ToastOverlay,
    /// Groupes d'onglets : gauche, droite (double panneau, F3).
    pub(crate) views: [adw::TabView; 2],
    side_boxes: [gtk::Box; 2],
    /// Côté actif (celui qui a le clavier).
    pub(crate) side: Cell<usize>,
    pub(crate) dual: Cell<bool>,
    dual_btn: gtk::ToggleButton,
    pub(crate) panes: RefCell<Vec<Rc<Pane>>>,
    pub(crate) preview: Rc<Preview>,
    pub(crate) sidebar: gtk::ListBox,
    side_items: RefCell<Vec<Option<SideItem>>>,
    pub local: FsRef,
    pub(crate) remotes: RefCell<HashMap<String, FsRef>>,
    pub(crate) hosts: RefCell<Vec<Host>>,
    pub(crate) discovering: Cell<bool>,
    pub(crate) bookmarks: RefCell<Bookmarks>,
    pub(crate) clip: RefCell<Option<Clip>>,
    pub(crate) transfers: Shared,
    pub(crate) tview: Rc<TransfersView>,
    pub(crate) show_hidden: Rc<Cell<bool>>,
    /// Opérations annulables (Ctrl+Z).
    pub(crate) history: RefCell<mysend::undo::History>,
    /// Affichage en grille (sinon en liste), retenu d'un lancement à l'autre.
    pub(crate) grid_mode: Rc<Cell<bool>>,
    view_btn: gtk::Button,
}

thread_local! {
    static WIN: RefCell<Weak<Win>> = RefCell::new(Weak::new());
}

/// La fenêtre (unique) ; les rappels des widgets passent par ici plutôt que de la capturer.
pub fn win() -> Rc<Win> {
    WIN.with(|w| w.borrow().upgrade()).expect("fenêtre")
}

pub fn try_win() -> Option<Rc<Win>> {
    WIN.with(|w| w.borrow().upgrade())
}

/// Choix liste / grille de l'interface graphique.
fn view_file() -> std::path::PathBuf {
    mysend::config::config_dir().join("gui-view")
}

/// Menu du clic droit dans la liste.
pub fn context_menu() -> gio::Menu {
    let m = gio::Menu::new();
    let item = |s: &gio::Menu, label: &str, action: &str, accel: Option<&str>| {
        let it = gio::MenuItem::new(Some(label), Some(action));
        if let Some(a) = accel {
            it.set_attribute_value("accel", Some(&a.to_variant()));
        }
        s.append_item(&it);
    };
    let s1 = gio::Menu::new();
    item(&s1, "Ouvrir", "win.open", Some("Return"));
    item(&s1, "Ouvrir un terminal ici", "win.terminal", Some("F4"));
    m.append_section(None, &s1);
    let s2 = gio::Menu::new();
    item(&s2, "Copier", "win.copy", Some("<Control>c"));
    item(&s2, "Couper", "win.cut", Some("<Control>x"));
    item(&s2, "Coller", "win.paste", Some("<Control>v"));
    item(&s2, "Copier le chemin", "win.copy-path", Some("<Control><Shift>c"));
    m.append_section(None, &s2);
    let s2b = gio::Menu::new();
    item(&s2b, "Copier vers l'autre panneau", "win.copy-other", Some("F6"));
    item(&s2b, "Déplacer vers l'autre panneau", "win.move-other", Some("<Shift>F6"));
    item(&s2b, "Comparer avec l'autre panneau…", "win.compare", Some("<Control>k"));
    item(&s2b, "Envoyer à un appareil Apple…", "win.airdrop", None);
    m.append_section(None, &s2b);
    let s3 = gio::Menu::new();
    item(&s3, "Renommer…", "win.rename", Some("F2"));
    item(&s3, "Nouveau dossier…", "win.new-folder", Some("<Control><Shift>n"));
    item(&s3, "Nouveau fichier…", "win.new-file", Some("<Control>n"));
    m.append_section(None, &s3);
    let s3b = gio::Menu::new();
    item(&s3b, "Compresser…", "win.compress", None);
    item(&s3b, "Extraire ici", "win.extract", None);
    m.append_section(None, &s3b);
    let s4 = gio::Menu::new();
    item(&s4, "Mettre à la corbeille", "win.trash", Some("Delete"));
    item(&s4, "Supprimer définitivement", "win.delete", Some("<Shift>Delete"));
    m.append_section(None, &s4);
    let s5 = gio::Menu::new();
    item(&s5, "Marque-page sur ce dossier…", "win.bookmark", Some("<Control>d"));
    m.append_section(None, &s5);
    m
}

impl Win {
    pub fn new(app: &adw::Application, hosts: Vec<String>, path: Option<String>, session: Option<Session>, send: Vec<String>, airdrop: bool) {
        let window = adw::ApplicationWindow::builder().application(app).title("mySend").default_width(1400).default_height(850).build();
        let toasts = adw::ToastOverlay::new();
        let views = [adw::TabView::new(), adw::TabView::new()];
        let tab_bars = [
            adw::TabBar::builder().view(&views[0]).autohide(false).build(),
            adw::TabBar::builder().view(&views[1]).autohide(false).build(),
        ];
        let side_boxes = [gtk::Box::new(gtk::Orientation::Vertical, 0), gtk::Box::new(gtk::Orientation::Vertical, 0)];
        for i in 0..2 {
            side_boxes[i].append(&tab_bars[i]);
            views[i].set_vexpand(true);
            side_boxes[i].append(&views[i]);
            side_boxes[i].add_css_class("side");
        }
        side_boxes[1].set_visible(false);
        let sides = gtk::Paned::new(gtk::Orientation::Horizontal);
        sides.set_start_child(Some(&side_boxes[0]));
        sides.set_end_child(Some(&side_boxes[1]));
        sides.set_resize_start_child(true);
        sides.set_resize_end_child(true);
        sides.set_shrink_start_child(false);
        sides.set_shrink_end_child(false);
        let dual_btn = gtk::ToggleButton::new();
        dual_btn.set_icon_name("view-dual-symbolic");
        dual_btn.set_tooltip_text(Some("Double panneau (F3)"));
        dual_btn.set_action_name(Some("win.dual"));
        let header = adw::HeaderBar::new();

        let back_btn = gtk::Button::from_icon_name("go-previous-symbolic");
        back_btn.set_tooltip_text(Some("Précédent (Alt+←)"));
        back_btn.set_action_name(Some("win.back"));
        let fwd_btn = gtk::Button::from_icon_name("go-next-symbolic");
        fwd_btn.set_tooltip_text(Some("Suivant (Alt+→)"));
        fwd_btn.set_action_name(Some("win.forward"));
        let up_btn = gtk::Button::from_icon_name("go-up-symbolic");
        up_btn.set_tooltip_text(Some("Dossier parent (Retour arrière)"));
        up_btn.set_action_name(Some("win.up"));
        let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.set_tooltip_text(Some("Rafraîchir (F5)"));
        refresh_btn.set_action_name(Some("win.refresh"));
        for b in [&back_btn, &fwd_btn, &up_btn, &refresh_btn] {
            header.pack_start(b);
        }

        let menu = gio::Menu::new();
        menu.append(Some("Annuler la dernière opération (Ctrl+Z)"), Some("win.undo"));
        menu.append(Some("Fichiers cachés"), Some("win.hidden"));
        menu.append(Some("Double panneau (F3)"), Some("win.dual"));
        menu.append(Some("Comparer, synchroniser… (Ctrl+K)"), Some("win.compare"));
        menu.append(Some("Marque-pages (Ctrl+B)"), Some("win.bookmarks"));
        menu.append(Some("Chercher dans les sous-dossiers (Ctrl+Maj+F)"), Some("win.find"));
        menu.append(Some("Corbeille"), Some("win.trash-view"));
        menu.append(Some("Continuer dans le terminal (Ctrl+G)"), Some("win.to-terminal"));
        menu.append(Some("Rechercher les machines"), Some("win.discover"));
        menu.append(Some("Raccourcis clavier"), Some("win.shortcuts"));
        let menu_btn = gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).build();
        header.pack_end(&menu_btn);
        let grid_saved = std::fs::read_to_string(view_file()).map(|t| t.trim() == "grid").unwrap_or(false);
        let view_btn = gtk::Button::from_icon_name(if grid_saved { "view-list-symbolic" } else { "view-grid-symbolic" });
        view_btn.set_tooltip_text(Some("Liste / grille (Ctrl+1 / Ctrl+2)"));
        view_btn.set_action_name(Some("win.view-toggle"));
        header.pack_end(&view_btn);
        header.pack_end(&dual_btn);
        let new_tab = gtk::Button::from_icon_name("tab-new-symbolic");
        new_tab.set_tooltip_text(Some("Nouvel onglet (Ctrl+T)"));
        new_tab.set_action_name(Some("win.new-tab"));
        header.pack_end(&new_tab);
        let term_btn = gtk::Button::from_icon_name("utilities-terminal-symbolic");
        term_btn.set_tooltip_text(Some("Terminal dans ce dossier (F4)"));
        term_btn.set_action_name(Some("win.terminal"));
        header.pack_end(&term_btn);

        let shared = transfers::new_shared();
        let tview = TransfersView::new(shared.clone());
        header.pack_end(&tview.button);

        let sidebar = gtk::ListBox::new();
        sidebar.add_css_class("navigation-sidebar");
        let side_sw = gtk::ScrolledWindow::builder().child(&sidebar).hscrollbar_policy(gtk::PolicyType::Never).build();
        side_sw.set_width_request(200);

        let preview = Preview::new();
        preview.clear("");
        let inner = gtk::Paned::new(gtk::Orientation::Horizontal);
        inner.set_start_child(Some(&sides));
        inner.set_end_child(Some(&preview.root));
        // l'aperçu garde sa largeur, la liste prend le reste
        inner.set_resize_start_child(true);
        inner.set_shrink_start_child(false);
        inner.set_resize_end_child(false);
        inner.set_shrink_end_child(false);
        let outer = gtk::Paned::new(gtk::Orientation::Horizontal);
        outer.set_start_child(Some(&side_sw));
        outer.set_end_child(Some(&inner));
        outer.set_resize_start_child(false);
        outer.set_shrink_start_child(false);
        outer.set_position(210);

        let tb = adw::ToolbarView::new();
        tb.add_top_bar(&header);
        tb.set_content(Some(&outer));
        tb.add_bottom_bar(&tview.revealer);
        toasts.set_child(Some(&tb));
        window.set_content(Some(&toasts));

        let w = Rc::new(Win {
            app: app.clone(),
            window,
            toasts,
            views,
            side_boxes,
            side: Cell::new(0),
            dual: Cell::new(false),
            dual_btn,
            panes: RefCell::default(),
            preview,
            sidebar,
            side_items: RefCell::default(),
            local: Arc::new(Fs::local()),
            remotes: RefCell::default(),
            hosts: RefCell::default(),
            discovering: Cell::new(false),
            bookmarks: RefCell::new(bookmarks::load()),
            clip: RefCell::new(None),
            transfers: shared,
            tview,
            show_hidden: Rc::new(Cell::new(false)),
            history: RefCell::default(),
            grid_mode: Rc::new(Cell::new(grid_saved)),
            view_btn,
        });
        WIN.with(|x| *x.borrow_mut() = Rc::downgrade(&w));
        w.actions();
        for (i, tb) in tab_bars.iter().enumerate() {
            w.tab_signals(i, tb);
        }
        w.sidebar_signals();
        crate::theme::install(|| {
            if let Some(w) = try_win() {
                w.pane_selection_changed_current();
            }
        });

        let local = w.local.clone();
        let start = path
            .map(|p| normpath(&join(&std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default(), &p)))
            .unwrap_or_else(|| local.home.clone());
        match session {
            Some(s) => w.restore_session(s),
            None if !send.is_empty() => w.start_send(send, airdrop),
            None => w.add_pane(local, start),
        }
        for h in hosts {
            w.open_host(Some(h.clone()), h, None);
        }
        w.rebuild_sidebar();
        w.start_discovery();
        let w2 = Rc::downgrade(&w);
        w.window.connect_close_request(move |_| {
            let Some(w) = w2.upgrade() else { return glib::Propagation::Proceed };
            if w.tview.active() {
                let w3 = w.clone();
                dialogs::confirm(&w.window, "Des transferts sont en cours", "Quitter quand même ? Ils seront annulés.", "Quitter", true, move || {
                    transfers::cancel_all(&w3.transfers);
                    w3.shutdown();
                    w3.window.destroy();
                });
                return glib::Propagation::Stop;
            }
            w.shutdown();
            glib::Propagation::Proceed
        });
        // garde la fenêtre en vie (le thread-local n'a qu'une référence faible)
        let keep = w.clone();
        w.window.connect_destroy(move |_| {
            let _ = &keep;
        });
        w.window.present();
    }

    /// Liste ou grille, pour tous les onglets ; le choix est retenu.
    pub(crate) fn set_grid(&self, on: bool) {
        self.grid_mode.set(on);
        for p in self.panes.borrow().iter() {
            p.set_grid();
        }
        self.view_btn.set_icon_name(if on { "view-list-symbolic" } else { "view-grid-symbolic" });
        let _ = std::fs::create_dir_all(mysend::config::config_dir());
        let _ = std::fs::write(view_file(), if on { "grid\n" } else { "list\n" });
    }

    /// --send : les fichiers sont dans le presse-papiers (copie) ; on ouvre leur dossier et le
    /// choix de la machine, il ne reste qu'à coller (Ctrl+V) au bon endroit.
    fn start_send(self: &Rc<Self>, paths: Vec<String>, airdrop: bool) {
        let items: Vec<(String, Entry)> = paths
            .iter()
            .filter_map(|p| {
                let mut e = self.local.stat(p).ok()?;
                e.is_link = std::fs::symlink_metadata(p).map(|m| m.file_type().is_symlink()).unwrap_or(false);
                Some((p.clone(), e))
            })
            .collect();
        let dir = paths.first().map(|p| dirname(p)).unwrap_or_else(|| self.local.home.clone());
        self.add_pane(self.local.clone(), dir);
        if items.is_empty() {
            return;
        }
        if airdrop {
            let w = self.clone();
            glib::idle_add_local_once(move || w.airdrop_items(w.local.clone(), items));
            return;
        }
        let n = items.len();
        *self.clip.borrow_mut() = Some(Clip { fs: self.local.clone(), items, cut: false });
        self.toast(&format!("{n} élément(s) à envoyer : choisis la machine, puis colle (Ctrl+V) dans le dossier voulu."));
        // le choix de la machine, une fois la recherche des machines finie (5 s au plus)
        let w = Rc::downgrade(self);
        let started = std::time::Instant::now();
        glib::timeout_add_local(Duration::from_millis(150), move || {
            let Some(w) = w.upgrade() else { return glib::ControlFlow::Break };
            if w.discovering.get() && started.elapsed() < Duration::from_secs(5) {
                return glib::ControlFlow::Continue;
            }
            dialogs::host_picker(&w.window, &w.hosts.borrow(), w.discovering.get());
            glib::ControlFlow::Break
        });
    }

    /// Ctrl+Z : annule la dernière opération (renommage, déplacement, corbeille, création, copie).
    pub(crate) fn undo_last(&self) {
        let Some(op) = self.history.borrow_mut().pop() else {
            self.toast("Rien à annuler.");
            return;
        };
        let what = op.describe();
        bg(
            move || op.undo(),
            move |r| {
                let w = win();
                match r {
                    Ok(touched) => {
                        w.toast(&format!("Annulé : {what}"));
                        w.refresh_fs(&touched);
                    }
                    Err(e) => w.error(&format!("Annulation impossible : {e}")),
                }
            },
        );
    }

    pub(crate) fn shutdown(&self) {
        for fs in self.remotes.borrow().values() {
            fs.close();
        }
    }

    pub fn toast(&self, msg: &str) {
        mysend::log::info(msg);
        let t = adw::Toast::new(&glib::markup_escape_text(msg));
        t.set_timeout(3);
        self.toasts.add_toast(t);
    }

    pub fn error(&self, msg: &str) {
        mysend::log::error(msg);
        let t = adw::Toast::new(&format!("<span foreground=\"{}\">{}</span>", crate::theme::current().red, glib::markup_escape_text(msg)));
        t.set_use_markup(true);
        t.set_timeout(8);
        self.toasts.add_toast(t);
    }

    // ---------------------------------------------------------------- onglets

    /// Groupe d'onglets du côté actif.
    pub(crate) fn tv(&self) -> &adw::TabView {
        &self.views[self.side.get()]
    }

    pub fn current(&self) -> Option<Rc<Pane>> {
        self.pane_on(self.side.get())
    }

    /// Onglet affiché d'un côté.
    pub(crate) fn pane_on(&self, side: usize) -> Option<Rc<Pane>> {
        let page = self.views[side].selected_page()?;
        self.pane_of_page(&page)
    }

    /// Côté où se trouve un onglet.
    pub(crate) fn side_of(&self, page: &adw::TabPage) -> usize {
        let v = &self.views[1];
        if (0..v.n_pages()).any(|i| &v.nth_page(i) == page) { 1 } else { 0 }
    }

    /// Affiche un onglet (de n'importe quel côté) et rend ce côté actif.
    pub(crate) fn select_page(&self, page: &adw::TabPage) {
        let side = self.side_of(page);
        self.views[side].set_selected_page(page);
        self.set_side(side);
    }

    /// Change le côté actif (surligné en double panneau).
    pub(crate) fn set_side(&self, side: usize) {
        if self.side.replace(side) != side {
            if let Some(p) = self.current() {
                self.pane_loaded(&p);
                self.pane_selection_changed(&p);
            }
        }
        for (i, b) in self.side_boxes.iter().enumerate() {
            if self.dual.get() && i == side {
                b.add_css_class("side-active");
            } else {
                b.remove_css_class("side-active");
            }
        }
    }

    /// F3 : double panneau. À l'ouverture, la droite montre le même dossier ; à la fermeture,
    /// ses onglets reviennent à gauche.
    pub(crate) fn set_dual(self: &Rc<Self>, on: bool) {
        if self.dual.get() == on {
            return;
        }
        self.dual.set(on);
        self.dual_btn.set_active(on);
        if on {
            self.side_boxes[1].set_visible(true);
            if self.views[1].n_pages() == 0 {
                if let Some(p) = self.pane_on(0) {
                    let (fs, cwd) = (p.fs.borrow().clone(), p.cwd.borrow().clone());
                    self.side.set(1);
                    self.add_pane(fs, cwd);
                }
            }
            self.set_side(1);
        } else {
            let (l, r) = (&self.views[0], &self.views[1]);
            while r.n_pages() > 0 {
                let page = r.nth_page(0);
                r.transfer_page(&page, l, l.n_pages());
            }
            self.side_boxes[1].set_visible(false);
            self.set_side(0);
        }
        if let Some(p) = self.current() {
            p.focus_list();
        }
    }

    /// Onglet affiché de l'autre côté (double panneau).
    pub(crate) fn other_pane(&self) -> Option<Rc<Pane>> {
        if !self.dual.get() {
            return None;
        }
        self.pane_on(1 - self.side.get())
    }

    /// F6 / Maj+F6 : copie / déplace la sélection dans le dossier de l'autre panneau.
    pub(crate) fn to_other_side(&self, cut: bool) {
        let (Some(p), Some(o)) = (self.current(), self.other_pane()) else {
            self.toast("Ouvre le double panneau (F3) pour copier d'un côté à l'autre.");
            return;
        };
        let items = p.selected();
        if items.is_empty() {
            return;
        }
        let (sfs, dfs, ddir) = (p.fs.borrow().clone(), o.fs.borrow().clone(), o.cwd.borrow().clone());
        if same(&sfs, &dfs) && ddir == *p.cwd.borrow() {
            self.toast("Les deux panneaux montrent le même dossier.");
            return;
        }
        self.transfer(sfs, items, cut, false, dfs, ddir);
    }

    pub(crate) fn pane_of_page(&self, page: &adw::TabPage) -> Option<Rc<Pane>> {
        self.panes.borrow().iter().find(|p| p.page.borrow().as_ref() == Some(page)).cloned()
    }

    pub fn add_pane(self: &Rc<Self>, fs: FsRef, path: String) {
        self.add_pane_at(fs, path, None);
    }

    /// Nouvel onglet sur `path` ; `select` : élément à mettre sous le curseur.
    pub(crate) fn add_pane_at(self: &Rc<Self>, fs: FsRef, path: String, select: Option<String>) {
        let pane = Pane::new(fs.clone(), self.show_hidden.clone(), self.grid_mode.clone());
        let page = self.tv().append(&pane.root);
        page.set_title(&fs.label);
        if fs.remote || fs.sudo.is_some() {
            page.set_icon(Some(&gio::ThemedIcon::new(if fs.sudo.is_some() { "security-high-symbolic" } else { "network-server-symbolic" })));
        }
        *pane.page.borrow_mut() = Some(page.clone());
        self.panes.borrow_mut().push(pane.clone());
        self.tv().set_selected_page(&page);
        pane.load(path, select);
        pane.focus_list();
    }

    // ---------------------------------------------------------------- terminal ↔ GTK

    /// Ctrl+G : continuer dans l'interface terminal (kitty), avec les mêmes onglets.
    pub(crate) fn to_terminal(self: &Rc<Self>) {
        if self.tview.active() {
            self.toast("Des transferts sont en cours : attends qu'ils finissent avant de changer d'interface.");
            return;
        }
        let mut s = Session::default();
        let selected = self.tv().selected_page();
        let pages: Vec<adw::TabPage> = self.views.iter().flat_map(|v| (0..v.n_pages()).map(|i| v.nth_page(i)).collect::<Vec<_>>()).collect();
        for page in pages {
            let Some(p) = self.pane_of_page(&page) else { continue };
            if Some(&page) == selected.as_ref() {
                s.active = s.tabs.len();
            }
            let sel = p.selected().into_iter().next().map(|(_, e)| e.name);
            s.push(&p.fs.borrow(), &p.cwd.borrow(), sel);
        }
        s.active = s.active.min(s.tabs.len().saturating_sub(1));
        let r = s.save().and_then(|f| {
            let argv = in_terminal(&[sibling("mysend"), "--session".into(), f.to_string_lossy().into_owned()]);
            mysend::proc::spawn_detached(&argv)
        });
        match r {
            Ok(()) => self.window.destroy(),
            Err(e) => self.error(&format!("Terminal impossible à lancer : {e} (variable TERMINAL)")),
        }
    }

    /// Rouvre les onglets d'une session venue de l'interface terminal.
    pub(crate) fn restore_session(self: &Rc<Self>, s: Session) {
        if s.skipped > 0 {
            self.toast(&format!("{} onglet(s) sudo non repris (mot de passe à redonner).", s.skipped));
        }
        let targets = s.targets();
        bg(
            move || connect_all(targets),
            move |conns| {
                let w = win();
                for (t, r) in conns {
                    match r {
                        Ok(fs) => {
                            w.remotes.borrow_mut().entry(t).or_insert_with(|| Arc::new(fs));
                        }
                        Err(e) => w.error(&format!("{t} : {e}")),
                    }
                }
                let mut pages = Vec::new();
                for t in s.tabs {
                    let fs = match &t.target {
                        None => Some(w.local.clone()),
                        Some(x) => w.remotes.borrow().get(x).cloned(),
                    };
                    match fs {
                        Some(fs) => {
                            w.add_pane_at(fs, t.path, t.select);
                            pages.push(w.tv().selected_page());
                        }
                        None => pages.push(None),
                    }
                }
                if w.panes.borrow().is_empty() {
                    let home = w.local.home.clone();
                    w.add_pane(w.local.clone(), home);
                } else if let Some(Some(page)) = pages.get(s.active) {
                    w.select_page(page);
                }
            },
        );
    }

    pub(crate) fn tab_signals(self: &Rc<Self>, side: usize, tab_bar: &adw::TabBar) {
        let view = &self.views[side];
        view.connect_close_page(|tv, page| {
            let w = win();
            if let Some(p) = w.pane_of_page(page) {
                p.dispose();
            }
            w.panes.borrow_mut().retain(|p| p.page.borrow().as_ref() != Some(page));
            tv.close_page_finish(page, true);
            if w.panes.borrow().is_empty() {
                w.window.close();
            } else if w.dual.get() && (w.views[0].n_pages() == 0 || w.views[1].n_pages() == 0) {
                // un côté vide : retour au panneau simple
                if w.views[0].n_pages() == 0 {
                    let (l, r) = (&w.views[0], &w.views[1]);
                    while r.n_pages() > 0 {
                        let p = r.nth_page(0);
                        r.transfer_page(&p, l, l.n_pages());
                    }
                }
                w.set_dual(false);
            }
            glib::Propagation::Stop
        });
        view.connect_selected_page_notify(move |_| {
            let w = win();
            let Some(p) = w.pane_on(side) else { return };
            if p.fs.borrow().remote {
                p.refresh();
            }
            w.set_side(side);
            w.pane_loaded(&p);
            w.pane_selection_changed(&p);
            p.focus_list();
        });
        // le clavier ou la souris dans un côté le rend actif
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter(move |_| {
            if let Some(w) = try_win() {
                w.set_side(side);
            }
        });
        self.side_boxes[side].add_controller(focus);
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.connect_pressed(move |_, _, _, _| {
            if let Some(w) = try_win() {
                w.set_side(side);
            }
        });
        self.side_boxes[side].add_controller(click);
        // déposer sur le titre d'un onglet : copie dans son dossier
        tab_bar.setup_extra_drop_target(gdk::DragAction::COPY, &[glib::BoxedAnyObject::static_type(), gdk::FileList::static_type()]);
        tab_bar.connect_extra_drag_drop(|_, page, value| {
            let w = win();
            match w.pane_of_page(page) {
                Some(p) => w.drop_into(&p, value),
                None => false,
            }
        });
    }

    /// Après un chargement : titres, boutons, aperçu.
    pub fn pane_loaded(&self, p: &Rc<Pane>) {
        if let Some(page) = p.page.borrow().as_ref() {
            page.set_title(&p.title());
            let fs = p.fs.borrow();
            page.set_tooltip(&glib::markup_escape_text(&fs.spec(&p.cwd.borrow())));
        }
        if self.current().map(|c| Rc::ptr_eq(&c, p)).unwrap_or(false) {
            let fs = p.fs.borrow();
            self.window.set_title(Some(&format!("{} — mySend", fs.spec(&fs.pretty(&p.cwd.borrow())))));
            let (b, f) = p.history();
            for (name, on) in [("back", b), ("forward", f)] {
                if let Some(a) = self.window.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                    a.set_enabled(on);
                }
            }
        }
    }

    pub fn pane_selection_changed(&self, p: &Rc<Pane>) {
        if !self.current().map(|c| Rc::ptr_eq(&c, p)).unwrap_or(false) {
            return;
        }
        let sel = p.selected();
        match sel.len() {
            0 => self.preview.clear(""),
            1 => {
                let (path, e) = sel.into_iter().next().unwrap();
                self.preview.show(p.fs.borrow().clone(), path, e);
            }
            n => self.preview.clear(&format!("{n} éléments sélectionnés")),
        }
    }

    pub(crate) fn pane_selection_changed_current(&self) {
        if let Some(p) = self.current() {
            self.pane_selection_changed(&p);
        }
    }

    /// Recharge les onglets qui montrent ces machines.
    pub fn refresh_fs(&self, list: &[FsRef]) {
        for fs in list {
            fs.clear_cache();
        }
        let panes = self.panes.borrow().clone();
        for p in panes {
            if list.iter().any(|f| same(f, &p.fs.borrow())) {
                p.refresh();
            }
        }
    }

    pub fn load_failed(self: &Rc<Self>, p: &Rc<Pane>, path: String, e: std::io::Error) {
        let msg = format!("{} : {}", path, errstr(&e));
        // on reste où l'on était
        let cwd = p.cwd.borrow().clone();
        p.path_entry.set_text(&p.fs.borrow().pretty(&cwd));
        p.update_status();
        if cwd.is_empty() {
            // premier chargement raté : on se rabat sur le dossier personnel
            let home = p.fs.borrow().home.clone();
            if path != home {
                p.load(home, None);
            }
        }
        if is_denied(&msg) {
            self.offer_elevation(p.fs.borrow().clone(), path);
        } else {
            self.error(&msg);
        }
    }

    // ---------------------------------------------------------------- machines

    pub fn open_host(self: &Rc<Self>, target: Option<String>, label: String, path: Option<String>) {
        let Some(target) = target else {
            let p = path.unwrap_or_else(|| self.local.home.clone());
            self.add_pane(self.local.clone(), p);
            return;
        };
        if let Some(fs) = self.remotes.borrow().get(&target).cloned() {
            let p = path.unwrap_or_else(|| fs.home.clone());
            self.add_pane(fs, p);
            return;
        }
        self.toast(&format!("Connexion à {label}…"));
        self.connect(target, label, path, None);
    }

    pub(crate) fn connect(self: &Rc<Self>, target: String, label: String, path: Option<String>, password: Option<String>) {
        enum R {
            Ok(Fs),
            Password,
            HostKey,
            Err(String),
        }
        let (t, l) = (target.clone(), label.clone());
        let with_pw = password.is_some();
        bg(
            move || {
                if let Some(pw) = password {
                    if let Err(e) = ssh_with_password(&t, &pw) {
                        return R::Err(e);
                    }
                } else {
                    // test de connexion (rouvre une connexion maîtresse morte au besoin)
                    let r = mysend::auth::check_ssh(&t);
                    let (ok, stderr) = match &r {
                        Ok(o) => (o.ok(), o.stderr.trim().to_string()),
                        Err(e) => (false, e.to_string()),
                    };
                    if !ok {
                        if stderr.contains("Host key verification failed") {
                            return R::HostKey;
                        }
                        if stderr.contains("Permission denied") || stderr.contains("passphrase") {
                            return R::Password;
                        }
                        return R::Err(if stderr.is_empty() { "Connexion impossible.".into() } else { stderr });
                    }
                }
                match Fs::connect_remote(&t, &l) {
                    Ok(fs) => R::Ok(fs),
                    Err(e) => R::Err(e.to_string()),
                }
            },
            move |r| {
                let w = win();
                match r {
                    R::Ok(fs) => {
                        let fs = w.remotes.borrow_mut().entry(target).or_insert_with(|| Arc::new(fs)).clone();
                        let p = path.unwrap_or_else(|| fs.home.clone());
                        w.add_pane(fs, p);
                    }
                    R::Password if !with_pw => {
                        let (t2, l2) = (target.clone(), label.clone());
                        dialogs::form(
                            &w.window,
                            &format!("Connexion à {label}"),
                            "Pas de clé ssh acceptée : mot de passe de ce compte.",
                            &[Field::Password("Mot de passe")],
                            "Se connecter",
                            None,
                            move |v| win().connect(t2, l2, path, Some(v[0].clone())),
                        );
                    }
                    R::Password => w.error(&format!("{label} : mot de passe refusé.")),
                    R::HostKey => {
                        let t2 = target.clone();
                        dialogs::confirm(
                            &w.window,
                            &format!("Clé inconnue pour {label}"),
                            "La machine n'est pas encore connue de ssh. Ouvrir un terminal pour vérifier son empreinte et l'accepter ? La connexion reprendra ensuite.",
                            "Ouvrir un terminal",
                            false,
                            move || {
                                let mut argv = vec!["ssh".to_string()];
                                argv.extend(mysend::config::ssh_opts());
                                argv.extend([t2.clone(), "true".into()]);
                                let cmd = in_terminal(&argv);
                                bg(
                                    move || std::process::Command::new(&cmd[0]).args(&cmd[1..]).status().is_ok(),
                                    move |ok| {
                                        if ok {
                                            win().connect(t2, label, path, None);
                                        }
                                    },
                                );
                            },
                        );
                    }
                    R::Err(e) => w.error(&format!("{label} : {e}")),
                }
            },
        );
    }

    pub(crate) fn start_discovery(self: &Rc<Self>) {
        if self.discovering.replace(true) {
            return;
        }
        bg(discover::discover, |hosts| {
            let w = win();
            *w.hosts.borrow_mut() = hosts;
            w.discovering.set(false);
            w.rebuild_sidebar();
        });
    }

    /// Accès refusé : rouvrir avec sudo, ou en tant qu'un autre utilisateur.
    pub fn offer_elevation(self: &Rc<Self>, fs: FsRef, path: String) {
        let d = adw::AlertDialog::new(
            Some("Accès refusé"),
            Some(&format!("{} sur {}.\nRouvrir ce dossier avec d'autres droits ?", path, fs.label)),
        );
        d.add_response("cancel", "Annuler");
        d.add_response("user", "Autre utilisateur…");
        d.add_response("sudo", "Avec sudo…");
        d.set_response_appearance("sudo", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("sudo"));
        d.set_close_response("cancel");
        d.connect_response(None, move |_, r| match r {
            "sudo" => win().elevate_sudo(fs.clone(), path.clone()),
            "user" => win().elevate_user(fs.clone(), path.clone()),
            _ => {}
        });
        d.present(Some(&self.window));
    }

    pub(crate) fn machine_label(fs: &Fs) -> String {
        match &fs.sudo {
            Some(s) => s.host.as_deref().map(|h| bare(h).to_string()).unwrap_or_else(|| "local".into()),
            None if fs.remote => bare(&fs.label).to_string(),
            None => "local".into(),
        }
    }

    pub(crate) fn elevate_sudo(self: &Rc<Self>, base: FsRef, path: String) {
        let host = base.host().map(String::from);
        let machine = Self::machine_label(&base);
        dialogs::form(
            &self.window,
            &format!("sudo sur {machine}"),
            "Devenir un autre utilisateur avec sudo. Le mot de passe est celui de ton compte.",
            &[Field::Text("Utilisateur", "root"), Field::Password("Mot de passe sudo")],
            "Ouvrir",
            None,
            move |v| {
                let user = if v[0].trim().is_empty() { "root".to_string() } else { v[0].trim().to_string() };
                let pw = v[1].clone();
                let key = format!("sudo:{user}@{}", host.as_deref().unwrap_or("local"));
                let w = win();
                if let Some(fs) = w.remotes.borrow().get(&key).cloned() {
                    w.add_pane(fs, path);
                    return;
                }
                let label = format!("{user}@{machine}(sudo)");
                w.toast(&format!("Connexion sudo ({label})…"));
                bg(
                    move || Fs::connect_sudo(host, &user, &pw, &label).map_err(|e| e.to_string()),
                    move |r| {
                        let w = win();
                        match r {
                            Ok(fs) => {
                                let fs = w.remotes.borrow_mut().entry(key).or_insert_with(|| Arc::new(fs)).clone();
                                w.add_pane(fs, path);
                            }
                            Err(e) => w.error(&e),
                        }
                    },
                );
            },
        );
    }

    pub(crate) fn elevate_user(self: &Rc<Self>, base: FsRef, path: String) {
        let host = base.host().map(|h| bare(h).to_string()).unwrap_or_else(|| "localhost".into());
        let machine = Self::machine_label(&base);
        dialogs::form(
            &self.window,
            &format!("Se connecter à {machine}"),
            "En tant qu'un autre utilisateur (connexion ssh par mot de passe).",
            &[Field::Text("Utilisateur", ""), Field::Password("Mot de passe")],
            "Se connecter",
            None,
            move |v| {
                let user = v[0].trim().to_string();
                if user.is_empty() {
                    return;
                }
                let target = format!("{user}@{host}");
                let label = format!("{user}@{machine}");
                let w = win();
                if let Some(fs) = w.remotes.borrow().get(&target).cloned() {
                    w.add_pane(fs, path);
                    return;
                }
                w.toast(&format!("Connexion à {label}…"));
                w.connect(target, label, Some(path), Some(v[1].clone()));
            },
        );
    }

    // ---------------------------------------------------------------- barre latérale

    pub(crate) fn rebuild_sidebar(&self) {
        while let Some(c) = self.sidebar.first_child() {
            self.sidebar.remove(&c);
        }
        let mut items: Vec<Option<SideItem>> = Vec::new();
        let title = |text: &str| {
            let l = gtk::Label::new(Some(text));
            l.add_css_class("sidebar-title");
            l.set_xalign(0.0);
            let r = gtk::ListBoxRow::new();
            r.set_child(Some(&l));
            r.set_activatable(false);
            r.set_selectable(false);
            r
        };
        let row = |icon: &str, text: &str, sub: Option<&str>, dot: Option<bool>| {
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            b.set_margin_start(4);
            let img = gtk::Image::from_icon_name(icon);
            b.append(&img);
            let l = gtk::Label::new(Some(text));
            l.set_xalign(0.0);
            l.set_hexpand(true);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            b.append(&l);
            if let Some(on) = dot {
                let d = gtk::Label::new(Some(if on { "●" } else { "○" }));
                d.add_css_class(if on { "host-on" } else { "host-off" });
                b.append(&d);
            }
            let r = gtk::ListBoxRow::new();
            r.set_child(Some(&b));
            if let Some(s) = sub {
                r.set_tooltip_text(Some(s));
            }
            r
        };
        self.sidebar.append(&title(if self.discovering.get() { "MACHINES  (recherche…)" } else { "MACHINES" }));
        items.push(None);
        self.sidebar.append(&row("computer-symbolic", "local", Some("cette machine"), Some(true)));
        items.push(Some(SideItem::Host(None, "local".into())));
        for h in self.hosts.borrow().iter() {
            let tip = format!("{}  ·  {}", h.target, h.address.clone().unwrap_or_else(|| "nom introuvable".into()));
            self.sidebar.append(&row("network-server-symbolic", &h.label(), Some(&tip), Some(h.reachable)));
            items.push(Some(SideItem::Host(Some(h.target.clone()), h.label())));
        }
        let bms = self.bookmarks.borrow();
        if !bms.is_empty() {
            self.sidebar.append(&title("MARQUE-PAGES"));
            items.push(None);
            for (k, b) in bms.iter() {
                let text = format!("{k}  {}", basename(&b.path).chars().take(40).collect::<String>());
                let tip = format!("{}:{}", b.label, b.path);
                self.sidebar.append(&row("user-bookmarks-symbolic", &text, Some(&tip), None));
                items.push(Some(SideItem::Bookmark(k.clone())));
            }
        }
        *self.side_items.borrow_mut() = items;
    }

    pub(crate) fn sidebar_signals(&self) {
        self.sidebar.connect_row_activated(|_, row| {
            let w = win();
            let idx = row.index() as usize;
            let item = match w.side_items.borrow().get(idx) {
                Some(Some(SideItem::Host(t, l))) => Some((t.clone(), l.clone(), None)),
                Some(Some(SideItem::Bookmark(k))) => w.bookmarks.borrow().get(k).map(|b| (b.target.clone(), b.label.clone(), Some(b.path.clone()))),
                _ => None,
            };
            if let Some((t, l, p)) = item {
                w.goto(t, l, p);
            }
        });
    }

    /// Va à `path` sur une machine : dans l'onglet courant s'il est sur cette machine, sinon
    /// dans un onglet déjà ouvert dessus, sinon un nouvel onglet.
    pub fn goto(self: &Rc<Self>, target: Option<String>, label: String, path: Option<String>) {
        let fs = match &target {
            None => Some(self.local.clone()),
            Some(t) => self.remotes.borrow().get(t).cloned(),
        };
        let Some(fs) = fs else {
            self.open_host(target, label, path);
            return;
        };
        let p = path.unwrap_or_else(|| fs.home.clone());
        if let Some(cur) = self.current() {
            if same(&cur.fs.borrow(), &fs) {
                cur.navigate(p);
                cur.focus_list();
                return;
            }
        }
        let other = self.panes.borrow().iter().find(|x| same(&x.fs.borrow(), &fs)).cloned();
        match other {
            Some(o) => {
                if let Some(page) = o.page.borrow().as_ref() {
                    self.select_page(page);
                }
                o.navigate(p);
            }
            None => self.add_pane(fs, p),
        }
    }

    /// Barre de chemin : « ~/x », « /x », ou « machine:/chemin ».
    pub fn goto_text(self: &Rc<Self>, p: &Rc<Pane>, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some((host, path)) = text.split_once(':') {
            if !host.is_empty() && !host.contains('/') {
                let path = if path.is_empty() { None } else { Some(path.to_string()) };
                if host == "local" {
                    self.goto(None, "local".into(), path);
                } else {
                    let target = self.hosts.borrow().iter().find(|h| h.label() == host).map(|h| h.target.clone()).unwrap_or(host.to_string());
                    self.goto(Some(target), host.to_string(), path);
                }
                return;
            }
        }
        let fs = p.fs.borrow().clone();
        let path = if text == "~" {
            fs.home.clone()
        } else if let Some(rest) = text.strip_prefix("~/") {
            join(&fs.home, rest)
        } else {
            normpath(&join(&p.cwd.borrow(), text))
        };
        p.navigate(path);
    }

    // ---------------------------------------------------------------- actions

    pub(crate) fn actions(self: &Rc<Self>) {
        let add = |name: &str, accels: &[&str], f: fn(&Rc<Win>)| {
            let a = gio::SimpleAction::new(name, None);
            a.connect_activate(move |_, _| f(&win()));
            self.window.add_action(&a);
            if !accels.is_empty() {
                self.app.set_accels_for_action(&format!("win.{name}"), accels);
            }
        };
        add("new-tab", &["<Control>t"], |w| dialogs::host_picker(&w.window, &w.hosts.borrow(), w.discovering.get()));
        add("close-tab", &["<Control>w"], |w| {
            if let Some(page) = w.tv().selected_page() {
                w.tv().close_page(&page);
            }
        });
        add("quit", &["<Control>q"], |w| {
            w.window.close();
        });
        add("location", &["<Control>l"], |w| {
            if let Some(p) = w.current() {
                p.path_entry.grab_focus();
            }
        });
        add("refresh", &["F5", "<Control>r"], |w| {
            if let Some(p) = w.current() {
                p.fs.borrow().clear_cache();
                p.refresh();
            }
        });
        add("back", &[], |w| {
            if let Some(p) = w.current() {
                p.go_back();
            }
        });
        add("forward", &[], |w| {
            if let Some(p) = w.current() {
                p.go_forward();
            }
        });
        add("up", &[], |w| {
            if let Some(p) = w.current() {
                p.go_up();
            }
        });
        add("terminal", &["F4"], |w| w.open_terminal());
        add("to-terminal", &["<Control>g"], |w| w.to_terminal());
        add("discover", &[], |w| {
            w.start_discovery();
            w.rebuild_sidebar();
        });
        add("shortcuts", &["F1", "<Control>question"], |w| dialogs::shortcuts(&w.window));
        add("bookmarks", &[], |w| w.bookmarks_dialog());
        add("view-list", &["<Control>1"], |w| w.set_grid(false));
        add("view-grid", &["<Control>2"], |w| w.set_grid(true));
        add("view-toggle", &[], |w| w.set_grid(!w.grid_mode.get()));
        add("dual", &["F3"], |w| w.set_dual(!w.dual.get()));
        add("airdrop", &[], |w| w.airdrop());
        add("undo", &[], |w| w.undo_last());
        add("compare", &["<Control>k"], |w| w.compare());
        add("copy-other", &["F6"], |w| w.to_other_side(false));
        add("move-other", &["<Shift>F6"], |w| w.to_other_side(true));
        add("find", &[], |w| {
            if let Some(p) = w.current() {
                w.recursive_search(&p, false);
            }
        });
        add("trash-view", &[], |w| {
            if let Some(p) = w.current() {
                w.trash_dialog(&p);
            }
        });
        add("compress", &[], |w| {
            if let Some(p) = w.current() {
                w.compress(&p);
            }
        });
        add("extract", &[], |w| {
            if let Some(p) = w.current() {
                w.extract(&p);
            }
        });
        add("open", &[], |w| {
            if let Some(p) = w.current() {
                if let Some((path, e)) = p.selected().into_iter().next() {
                    w.open_entry(&p, path, e);
                }
            }
        });
        add("copy", &[], |w| w.yank(false));
        add("cut", &[], |w| w.yank(true));
        add("paste", &[], |w| w.paste(false));
        add("copy-path", &[], |w| w.copy_path());
        add("rename", &[], |w| w.rename());
        add("new-folder", &[], |w| w.create(true));
        add("new-file", &[], |w| w.create(false));
        add("trash", &[], |w| w.delete(false));
        add("delete", &[], |w| w.delete(true));
        add("bookmark", &[], |w| w.add_bookmark());

        let hidden = gio::SimpleAction::new_stateful("hidden", None, &false.to_variant());
        hidden.connect_activate(|a, _| {
            let w = win();
            let v = !w.show_hidden.get();
            w.show_hidden.set(v);
            a.set_state(&v.to_variant());
            for p in w.panes.borrow().iter() {
                p.set_hidden_changed();
            }
        });
        self.window.add_action(&hidden);
        self.app.set_accels_for_action("win.hidden", &["<Control>h"]);
    }

    // ---------------------------------------------------------------- opérations

    pub(crate) fn yank(&self, cut: bool) {
        let Some(p) = self.current() else { return };
        let items = p.selected();
        if items.is_empty() {
            return;
        }
        let n = items.len();
        *self.clip.borrow_mut() = Some(Clip { fs: p.fs.borrow().clone(), items, cut });
        self.redecorate_all();
        self.toast(&format!("{n} élément(s) {} — Ctrl+V pour coller", if cut { "coupé(s)" } else { "copié(s)" }));
    }

    pub(crate) fn paste(self: &Rc<Self>, overwrite: bool) {
        let Some(p) = self.current() else { return };
        let Some(clip) = self.clip.borrow_mut().take() else {
            self.toast("Presse-papiers vide : Ctrl+C pour copier, Ctrl+X pour couper.");
            return;
        };
        let (dfs, ddir) = (p.fs.borrow().clone(), p.cwd.borrow().clone());
        let cut = clip.cut;
        self.transfer(clip.fs.clone(), clip.items.clone(), cut, overwrite, dfs, ddir);
        if !cut {
            *self.clip.borrow_mut() = Some(clip);
        }
        self.redecorate_all();
    }

    /// `Some(cut)` si `path` de `fs` est dans le presse-papiers.
    pub fn yank_state(&self, fs: &FsRef, path: &str) -> Option<bool> {
        let c = self.clip.try_borrow().ok()?;
        let c = c.as_ref()?;
        (same(&c.fs, fs) && c.items.iter().any(|(p, _)| p == path)).then_some(c.cut)
    }

    pub(crate) fn redecorate_all(&self) {
        for p in self.panes.borrow().iter() {
            p.redecorate();
        }
    }

    pub(crate) fn transfer(&self, sfs: FsRef, items: Vec<(String, Entry)>, cut: bool, overwrite: bool, dfs: FsRef, ddir: String) {
        let n = items.len();
        let title = format!(
            "{} de {n} élément(s) de {} vers {}:{}",
            if cut { "Déplacement" } else { "Copie" },
            sfs.label,
            dfs.label,
            dfs.pretty(&ddir)
        );
        let dest = if ddir == dfs.home { "~".to_string() } else { basename(&ddir).to_string() };
        let short = format!("{n} élément(s) {} vers {} : {dest}", if cut { "déplacé(s)" } else { "copié(s)" }, dfs.label);
        let task = transfers::paste_task(title, sfs, items, cut, overwrite, dfs, ddir);
        transfers::enqueue(&self.transfers, task, move |d: Done| {
            on_main(move || {
                let w = win();
                mysend::notify::transfer_done(&d);
                match d.result {
                    Ok(()) => w.toast(&short),
                    Err(msg) => w.error(&format!("{} : {msg}", d.title)),
                }
                w.history.borrow_mut().push(mysend::undo::UndoOp::Batch(d.undo));
                w.refresh_fs(&d.touches);
            })
        });
    }

    /// Déposer des éléments (glisser interne, ou fichiers du bureau) dans le dossier de `p`.
    pub fn drop_into(&self, p: &Rc<Pane>, value: &glib::Value) -> bool {
        let (dfs, ddir) = (p.fs.borrow().clone(), p.cwd.borrow().clone());
        let (sfs, items) = if let Ok(b) = value.get::<glib::BoxedAnyObject>() {
            let d = b.borrow::<DragItems>().clone();
            (d.fs, d.items)
        } else if let Ok(list) = value.get::<gdk::FileList>() {
            let items: Vec<(String, Entry)> = list
                .files()
                .iter()
                .filter_map(|f| f.path())
                .filter_map(|pb| {
                    let path = pb.to_string_lossy().into_owned();
                    let mut e = self.local.stat(&path).ok()?;
                    e.is_link = std::fs::symlink_metadata(&pb).map(|m| m.file_type().is_symlink()).unwrap_or(false);
                    Some((path, e))
                })
                .collect();
            (self.local.clone(), items)
        } else {
            return false;
        };
        // déposé là d'où ça vient : rien à faire
        let here = same(&sfs, &dfs);
        let items: Vec<(String, Entry)> = items.into_iter().filter(|(path, _)| !(here && dirname(path) == ddir)).collect();
        if items.is_empty() {
            return false;
        }
        self.transfer(sfs, items, false, false, dfs, ddir);
        true
    }

    pub(crate) fn copy_path(&self) {
        let Some(p) = self.current() else { return };
        let fs = p.fs.borrow().clone();
        let mut items: Vec<String> = p.selected().into_iter().map(|(path, _)| fs.spec(&path)).collect();
        if items.is_empty() {
            items.push(fs.spec(&p.cwd.borrow()));
        }
        let text = items.join("\n");
        self.window.clipboard().set_text(&text);
        self.toast(&format!("Copié : {text}"));
    }

    pub(crate) fn create(self: &Rc<Self>, dir: bool) {
        let Some(p) = self.current() else { return };
        let heading = if dir { "Nouveau dossier" } else { "Nouveau fichier" };
        let pw = Rc::downgrade(&p);
        dialogs::form(&self.window, heading, "", &[Field::Text("Nom", "")], "Créer", None, move |v| {
            let Some(p) = pw.upgrade() else { return };
            let name = v[0].trim().trim_end_matches('/').to_string();
            if !name.is_empty() {
                win().create_named(&p, if dir { format!("{name}/") } else { name });
            }
        });
    }

    /// Crée `name` dans le dossier de `p` : un dossier si le nom finit par « / ».
    pub(crate) fn create_named(self: &Rc<Self>, p: &Rc<Pane>, name: String) {
        let (fs, cwd) = (p.fs.borrow().clone(), p.cwd.borrow().clone());
        let path = normpath(&join(&cwd, &name));
        let dir = name.ends_with('/');
        let first = mysend::util::relpath(&path, &cwd).split('/').next().unwrap_or("").to_string();
        let p = p.clone();
        let op = mysend::undo::UndoOp::Create { fs: fs.clone(), path: path.clone() };
        bg(
            move || {
                if fs.exists(&path) {
                    Err(std::io::Error::other("existe déjà"))
                } else if dir {
                    fs.makedirs(&path)
                } else {
                    fs.makedirs(&dirname(&path)).and_then(|_| fs.touch(&path))
                }
            },
            move |r| match r {
                Ok(()) => {
                    win().history.borrow_mut().push(op);
                    p.load(p.cwd.borrow().clone(), Some(first))
                }
                Err(e) => win().error(&format!("{name} : {}", errstr(&e))),
            },
        );
    }

    pub(crate) fn rename(self: &Rc<Self>) {
        let Some(p) = self.current() else { return };
        let sel = p.selected();
        let [(src, e)] = sel.as_slice() else {
            if sel.len() > 1 {
                self.toast("Renommer : un seul élément à la fois.");
            }
            return;
        };
        let (src, e) = (src.clone(), e.clone());
        let stem = match e.name.rfind('.') {
            Some(i) if i > 0 && !e.is_dir => e.name[..i].chars().count() as i32,
            _ => -1,
        };
        let pw = Rc::downgrade(&p);
        let cur = e.name.clone();
        dialogs::form(&self.window, "Renommer", "", &[Field::Text("Nouveau nom", &cur)], "Renommer", Some((0, stem)), move |v| {
            let Some(p) = pw.upgrade() else { return };
            let new = v[0].trim().to_string();
            if new.is_empty() || new == e.name {
                return;
            }
            let fs = p.fs.borrow().clone();
            let dst = normpath(&join(&dirname(&src), &new));
            let sel = basename(&dst).to_string();
            let op = mysend::undo::UndoOp::Rename { fs: fs.clone(), from: src.clone(), to: dst.clone() };
            bg(
                move || fs.rename(&src, &dst),
                move |r| match r {
                    Ok(()) => {
                        win().history.borrow_mut().push(op);
                        p.load(p.cwd.borrow().clone(), Some(sel))
                    }
                    Err(err) => win().error(&format!("{new} : {}", errstr(&err))),
                },
            );
        });
    }

    pub(crate) fn delete(self: &Rc<Self>, definitive: bool) {
        let Some(p) = self.current() else { return };
        let items = p.selected();
        if items.is_empty() {
            return;
        }
        let fs = p.fs.borrow().clone();
        let what = if items.len() == 1 { format!("« {} »", items[0].1.name) } else { format!("{} éléments", items.len()) };
        let (heading, body, ok) = if definitive {
            ("Supprimer définitivement ?", format!("{what} sur {} sera supprimé, sans passer par la corbeille.", fs.label), "Supprimer")
        } else {
            ("Mettre à la corbeille ?", format!("{what} sur {}.", fs.label), "Corbeille")
        };
        dialogs::confirm(&self.window, heading, &body, ok, true, move || {
            let paths: Vec<String> = items.into_iter().map(|(p, _)| p).collect();
            let fs2 = fs.clone();
            bg(
                move || {
                    let mut errs = Vec::new();
                    let mut done = Vec::new();
                    for p in &paths {
                        let r = if definitive { fs2.remove(p) } else { mysend::trash::put(&fs2, p) };
                        match r {
                            Err(e) => errs.push(format!("{} : {}", basename(p), errstr(&e))),
                            Ok(()) if !definitive => done.push(mysend::undo::UndoOp::Trash { fs: fs2.clone(), original: p.clone() }),
                            Ok(()) => {}
                        }
                    }
                    (errs, done)
                },
                move |(errs, done)| {
                    let w = win();
                    w.history.borrow_mut().push(mysend::undo::UndoOp::Batch(done));
                    if !errs.is_empty() {
                        w.error(&errs.join("\n"));
                    }
                    w.refresh_fs(&[fs]);
                },
            );
        });
    }

    pub(crate) fn add_bookmark(self: &Rc<Self>) {
        let Some(p) = self.current() else { return };
        let fs = p.fs.borrow().clone();
        let path = p.cwd.borrow().clone();
        let free = ('a'..='z').find(|c| !self.bookmarks.borrow().contains_key(&c.to_string())).map(|c| c.to_string()).unwrap_or_default();
        dialogs::form(
            &self.window,
            "Marque-page",
            &format!("{}:{}\nLa lettre sert aussi dans l'app terminal (' puis la lettre).", fs.label, fs.pretty(&path)),
            &[Field::Text("Lettre", &free)],
            "Ajouter",
            None,
            move |v| {
                let k = v[0].trim().to_string();
                if k.is_empty() {
                    return;
                }
                let w = win();
                let bm = Bookmark { target: if fs.remote { Some(fs.target.clone()) } else { None }, label: fs.label.clone(), path };
                w.bookmarks.borrow_mut().insert(k, bm);
                bookmarks::save(&w.bookmarks.borrow());
                w.rebuild_sidebar();
            },
        );
    }

    pub(crate) fn open_terminal(&self) {
        let Some(p) = self.current() else { return };
        let fs = p.fs.borrow().clone();
        let (argv, cwd) = fs.shell_argv(&p.cwd.borrow());
        let cmd = if fs.remote || fs.sudo.is_some() { in_terminal(&argv) } else { crate::util::terminal_here(&cwd) };
        let r = std::process::Command::new(&cmd[0])
            .args(&cmd[1..])
            .current_dir(&cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match r {
            Ok(mut c) => {
                std::thread::spawn(move || c.wait());
            }
            Err(e) => self.error(&format!("{} : {e} (variable TERMINAL)", cmd[0])),
        }
    }

    /// Entrée / double-clic : dossier → y entrer ; fichier → application par défaut.
    /// Un fichier distant est téléchargé, ouvert, et renvoyé à chaque enregistrement.
    pub fn open_entry(self: &Rc<Self>, p: &Rc<Pane>, path: String, e: Entry) {
        if e.is_dir {
            p.navigate(path);
            return;
        }
        let fs = p.fs.borrow().clone();
        if !fs.remote && fs.sudo.is_none() {
            if let Err(err) = gio::AppInfo::launch_default_for_uri(&gio::File::for_path(&path).uri(), None::<&gio::AppLaunchContext>) {
                self.error(&format!("{} : {err}", e.name));
            }
            return;
        }
        self.toast(&format!("Téléchargement de {}…", e.name));
        let fs2 = fs.clone();
        let p2 = path.clone();
        bg(
            move || {
                mkdtemp("open-").and_then(|d| {
                    let l = d.join(basename(&p2));
                    fs2.get(&p2, &l).map(|_| l)
                })
            },
            move |r| {
                let w = win();
                match r {
                    Ok(local) => {
                        let uri = gio::File::for_path(&local).uri();
                        if let Err(err) = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>) {
                            w.error(&format!("{} : {err}", e.name));
                            return;
                        }
                        w.watch_upload(fs, path, local);
                    }
                    Err(err) => w.error(&format!("{path} : {}", errstr(&err))),
                }
            },
        );
    }

    /// Renvoie la copie locale d'un fichier distant sur la machine à chaque modification
    /// (pendant 12 h au plus).
    pub(crate) fn watch_upload(&self, fs: FsRef, remote: String, local: std::path::PathBuf) {
        let mtime = |l: &std::path::Path| std::fs::metadata(l).and_then(|m| m.modified()).ok();
        let last = Cell::new(mtime(&local));
        let started = SystemTime::now();
        glib::timeout_add_local(Duration::from_secs(2), move || {
            if started.elapsed().map(|d| d > Duration::from_secs(12 * 3600)).unwrap_or(true) || !local.exists() {
                return glib::ControlFlow::Break;
            }
            let now = mtime(&local);
            if now != last.get() {
                last.set(now);
                let (fs, remote, local) = (fs.clone(), remote.clone(), local.clone());
                let r2 = remote.clone();
                bg(
                    move || fs.put(&local, &r2).map(|_| fs),
                    move |r| {
                        let w = win();
                        match r {
                            Ok(fs) => {
                                w.toast(&format!("« {} » renvoyé sur {}", basename(&remote), fs.label));
                                w.refresh_fs(&[fs]);
                            }
                            Err(e) => w.error(&format!("{remote} : {}", errstr(&e))),
                        }
                    },
                );
            }
            glib::ControlFlow::Continue
        });
    }
}
