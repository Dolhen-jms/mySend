//! Un onglet : un dossier sur une machine, affiché en liste à colonnes.

use crate::util::{bg, boxed, entry_of, icon_for};
use crate::window::win;
use gtk::{gdk, gio, glib, pango, prelude::*};
use mysend::fs::{Entry, FsRef};
use mysend::util::{basename, dirname, filemode, fmt_time, human, join, natural_cmp};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

/// Contenu d'un glisser interne : des éléments d'une machine.
#[derive(Clone)]
pub struct DragItems {
    pub fs: FsRef,
    pub items: Vec<(String, Entry)>,
}

pub struct Pane {
    pub fs: RefCell<FsRef>,
    pub cwd: RefCell<String>,
    back: RefCell<Vec<String>>,
    fwd: RefCell<Vec<String>>,
    generation: Cell<u64>,
    pub root: gtk::Box,
    pub path_entry: gtk::Entry,
    pub search_bar: gtk::SearchBar,
    search: gtk::SearchEntry,
    pub store: gio::ListStore,
    filter: gtk::CustomFilter,
    pub selection: gtk::MultiSelection,
    pub view: gtk::ColumnView,
    /// Vue en grille (mêmes éléments, même sélection) ; `stack` montre l'une ou l'autre.
    pub grid: gtk::GridView,
    stack: gtk::Stack,
    grid_mode: Rc<Cell<bool>>,
    status: gtk::Label,
    pub page: RefCell<Option<adw::TabPage>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    menu: RefCell<Option<gtk::PopoverMenu>>,
    /// Lignes affichées : nom → cellule du nom (pour la marque copié / coupé).
    bound: RefCell<HashMap<String, Vec<glib::WeakRef<gtk::Box>>>>,
}

fn label_cell(xalign: f32, muted: bool) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_xalign(xalign);
    if muted {
        l.add_css_class("muted");
    }
    l
}

impl Pane {
    pub fn new(fs: FsRef, show_hidden: Rc<Cell<bool>>, grid_mode: Rc<Cell<bool>>) -> Rc<Pane> {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Filtrer ce dossier…"));
        let search_bar = gtk::SearchBar::builder().child(&search).show_close_button(true).build();
        search_bar.connect_entry(&search);

        let s2 = search.clone();
        let filter = gtk::CustomFilter::new(move |obj| {
            let e = entry_of(obj);
            if !show_hidden.get() && e.name.starts_with('.') {
                return false;
            }
            let q = s2.text().to_lowercase();
            q.is_empty() || e.name.to_lowercase().contains(&q)
        });
        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));

        let view = gtk::ColumnView::new(None::<gtk::MultiSelection>);
        view.set_vexpand(true);
        view.set_show_row_separators(false);
        view.add_css_class("data-table");
        // dossiers d'abord, puis le tri choisi en cliquant sur les en-têtes
        let dirs_first = gtk::CustomSorter::new(|a, b| {
            let (a, b) = (entry_of(a), entry_of(b));
            b.is_dir.cmp(&a.is_dir).into()
        });
        let sorter = gtk::MultiSorter::new();
        sorter.append(dirs_first);
        if let Some(s) = view.sorter() {
            sorter.append(s);
        }
        let sorted = gtk::SortListModel::new(Some(filtered), Some(sorter));
        let selection = gtk::MultiSelection::new(Some(sorted));
        view.set_model(Some(&selection));

        let path_entry = gtk::Entry::new();
        path_entry.set_hexpand(true);
        path_entry.set_tooltip_text(Some("Chemin (Ctrl+L) ; « machine:/chemin » pour une autre machine"));
        let path_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        path_bar.add_css_class("path-bar");
        path_bar.append(&path_entry);

        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.add_css_class("pane-status");
        status.set_ellipsize(pango::EllipsizeMode::End);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_width_request(360);
        root.append(&path_bar);
        root.append(&search_bar);
        let grid = gtk::GridView::new(None::<gtk::MultiSelection>, None::<gtk::ListItemFactory>);
        grid.set_model(Some(&selection));
        grid.set_max_columns(30);
        grid.set_min_columns(2);
        grid.set_enable_rubberband(true);
        grid.add_css_class("file-grid");
        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.add_named(&gtk::ScrolledWindow::builder().child(&view).build(), Some("list"));
        stack.add_named(&gtk::ScrolledWindow::builder().child(&grid).hscrollbar_policy(gtk::PolicyType::Never).build(), Some("grid"));
        stack.set_visible_child_name(if grid_mode.get() { "grid" } else { "list" });
        root.append(&stack);
        root.append(&status);

        let pane = Rc::new(Pane {
            fs: RefCell::new(fs),
            cwd: RefCell::new(String::new()),
            back: RefCell::default(),
            fwd: RefCell::default(),
            generation: Cell::new(0),
            root,
            path_entry,
            search_bar,
            search,
            store,
            filter,
            selection,
            view,
            grid,
            stack,
            grid_mode,
            status,
            page: RefCell::new(None),
            monitor: RefCell::new(None),
            menu: RefCell::new(None),
            bound: RefCell::default(),
        });
        pane.columns();
        pane.grid_factory();
        pane.signals();
        pane
    }

    fn columns(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        // Nom
        let f = gtk::SignalListItemFactory::new();
        f.connect_setup(move |_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let img = gtk::Image::new();
            img.set_pixel_size(18);
            let lbl = gtk::Label::new(None);
            lbl.set_xalign(0.0);
            lbl.set_ellipsize(pango::EllipsizeMode::Middle);
            b.append(&img);
            b.append(&lbl);
            item.set_child(Some(&b));
            Pane::row_controllers(&weak, item, &b);
        });
        let weak = Rc::downgrade(self);
        f.connect_unbind(move |_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let (Some(me), Some(b), Some(o)) = (weak.upgrade(), item.child().and_downcast::<gtk::Box>(), item.item()) else { return };
            me.unbound(&entry_of(&o).name, &b);
        });
        let weak = Rc::downgrade(self);
        f.connect_bind(move |_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let (Some(b), Some(o)) = (item.child().and_downcast::<gtk::Box>(), item.item()) else { return };
            if let Some(me) = weak.upgrade() {
                let name = entry_of(&o).name;
                me.decorate(&b, &name);
                me.bound.borrow_mut().entry(name).or_default().push(b.downgrade());
            }
            let e = entry_of(&o);
            let img = b.first_child().and_downcast::<gtk::Image>().unwrap();
            let lbl = img.next_sibling().and_downcast::<gtk::Label>().unwrap();
            img.set_from_gicon(&icon_for(&e));
            lbl.set_text(&e.name);
            lbl.set_css_classes(&[]);
            if e.is_dir {
                lbl.add_css_class("dir-name");
            }
            if e.is_link {
                lbl.add_css_class("link-name");
            }
        });
        let name_sorter = gtk::CustomSorter::new(|a, b| natural_cmp(&entry_of(a).name, &entry_of(b).name).into());
        let col = gtk::ColumnViewColumn::new(Some("Nom"), Some(f));
        col.set_expand(true);
        col.set_resizable(true);
        col.set_sorter(Some(&name_sorter));
        self.view.append_column(&col);
        self.view.sort_by_column(Some(&col), gtk::SortType::Ascending);

        let simple = |title: &str, xalign: f32, width: i32, text: fn(&Entry) -> String, sorter: gtk::CustomSorter| {
            let f = gtk::SignalListItemFactory::new();
            let weak = Rc::downgrade(self);
            f.connect_setup(move |_, obj| {
                let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
                let l = label_cell(xalign, true);
                item.set_child(Some(&l));
                Pane::row_controllers(&weak, item, &l);
            });
            f.connect_bind(move |_, obj| {
                let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
                if let (Some(l), Some(o)) = (item.child().and_downcast::<gtk::Label>(), item.item()) {
                    l.set_text(&text(&entry_of(&o)));
                }
            });
            let col = gtk::ColumnViewColumn::new(Some(title), Some(f));
            col.set_fixed_width(width);
            col.set_resizable(true);
            col.set_sorter(Some(&sorter));
            col
        };
        self.view.append_column(&simple(
            "Taille",
            1.0,
            100,
            |e| if e.is_dir { String::new() } else { human(e.size) },
            gtk::CustomSorter::new(|a, b| entry_of(a).size.cmp(&entry_of(b).size).into()),
        ));
        self.view.append_column(&simple(
            "Modifié",
            0.0,
            150,
            |e| fmt_time(e.mtime, "%d/%m/%Y %H:%M"),
            gtk::CustomSorter::new(|a, b| entry_of(a).mtime.total_cmp(&entry_of(b).mtime).into()),
        ));
        self.view.append_column(&simple(
            "Droits",
            0.0,
            110,
            |e| filemode(e.mode),
            gtk::CustomSorter::new(|a, b| entry_of(a).mode.cmp(&entry_of(b).mode).into()),
        ));
    }

    /// Cases de la grille : grande icône, nom sur deux lignes.
    fn grid_factory(self: &Rc<Self>) {
        let f = gtk::SignalListItemFactory::new();
        let weak = Rc::downgrade(self);
        f.connect_setup(move |_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
            b.add_css_class("grid-cell");
            let img = gtk::Image::new();
            img.set_pixel_size(72);
            let lbl = gtk::Label::new(None);
            lbl.set_wrap(true);
            lbl.set_wrap_mode(pango::WrapMode::WordChar);
            lbl.set_lines(2);
            lbl.set_ellipsize(pango::EllipsizeMode::Middle);
            lbl.set_justify(gtk::Justification::Center);
            lbl.set_max_width_chars(14);
            lbl.set_width_chars(14);
            b.append(&img);
            b.append(&lbl);
            item.set_child(Some(&b));
            Pane::row_controllers(&weak, item, &b);
        });
        let weak = Rc::downgrade(self);
        f.connect_unbind(move |_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let (Some(me), Some(b), Some(o)) = (weak.upgrade(), item.child().and_downcast::<gtk::Box>(), item.item()) else { return };
            me.unbound(&entry_of(&o).name, &b);
        });
        let weak = Rc::downgrade(self);
        f.connect_bind(move |_, obj| {
            let item = obj.downcast_ref::<gtk::ListItem>().unwrap();
            let (Some(b), Some(o)) = (item.child().and_downcast::<gtk::Box>(), item.item()) else { return };
            let e = entry_of(&o);
            let img = b.first_child().and_downcast::<gtk::Image>().unwrap();
            let lbl = img.next_sibling().and_downcast::<gtk::Label>().unwrap();
            img.set_widget_name("");
            img.set_from_gicon(&icon_for(&e));
            lbl.set_text(&e.name);
            lbl.set_css_classes(&[]);
            if e.is_dir {
                lbl.add_css_class("dir-name");
            }
            if e.is_link {
                lbl.add_css_class("link-name");
            }
            b.set_tooltip_text(Some(&e.name));
            if let Some(me) = weak.upgrade() {
                // image : miniature (l'icône reste affichée en attendant)
                if crate::thumbs::is_image(&e) {
                    let path = join(&me.cwd.borrow(), &e.name);
                    crate::thumbs::load(&img, me.fs.borrow().clone(), path, &e);
                }
                me.decorate(&b, &e.name);
                me.bound.borrow_mut().entry(e.name).or_default().push(b.downgrade());
            }
        });
        self.grid.set_factory(Some(&f));
    }

    fn unbound(&self, name: &str, b: &gtk::Box) {
        let mut bound = self.bound.borrow_mut();
        if let Some(v) = bound.get_mut(name) {
            v.retain(|w| w.upgrade().map(|x| &x != b).unwrap_or(false));
            if v.is_empty() {
                bound.remove(name);
            }
        }
    }

    /// Vue affichée : liste ou grille.
    fn active_view(&self) -> gtk::Widget {
        if self.grid_mode.get() { self.grid.clone().upcast() } else { self.view.clone().upcast() }
    }

    /// Fait défiler la vue affichée jusqu'à `pos` (et la sélectionne / y met le focus selon `flags`).
    pub fn scroll_to(&self, pos: u32, flags: gtk::ListScrollFlags) {
        if self.grid_mode.get() {
            self.grid.scroll_to(pos, flags, None);
        } else {
            self.view.scroll_to(pos, None, flags, None);
        }
    }

    /// Bascule liste / grille (le focus suit).
    pub fn set_grid(&self) {
        self.stack.set_visible_child_name(if self.grid_mode.get() { "grid" } else { "list" });
        if let Some(pos) = (0..self.selection.n_items()).find(|&i| self.selection.is_selected(i)) {
            self.scroll_to(pos, gtk::ListScrollFlags::FOCUS);
        }
        self.focus_list();
    }

    /// Sur chaque cellule : glisser les éléments, clic droit qui sélectionne la ligne.
    fn row_controllers(weak: &Weak<Pane>, item: &gtk::ListItem, w: &impl IsA<gtk::Widget>) {
        let ds = gtk::DragSource::new();
        ds.set_actions(gdk::DragAction::COPY);
        let (wk, it) = (weak.clone(), item.downgrade());
        ds.connect_prepare(move |_, _, _| {
            let (me, item) = (wk.upgrade()?, it.upgrade()?);
            let pos = item.position();
            if !me.selection.is_selected(pos) {
                me.selection.select_item(pos, true);
            }
            let items = me.selected();
            if items.is_empty() {
                return None;
            }
            let fs = me.fs.borrow().clone();
            let mut providers =
                vec![gdk::ContentProvider::for_value(&glib::BoxedAnyObject::new(DragItems { fs: fs.clone(), items: items.clone() }).to_value())];
            if !fs.remote {
                let files: Vec<gio::File> = items.iter().map(|(p, _)| gio::File::for_path(p)).collect();
                providers.push(gdk::ContentProvider::for_value(&gdk::FileList::from_array(&files).to_value()));
            }
            Some(gdk::ContentProvider::new_union(&providers))
        });
        w.add_controller(ds);

        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let (wk, it) = (weak.clone(), item.downgrade());
        click.connect_pressed(move |_, _, _, _| {
            let (Some(me), Some(item)) = (wk.upgrade(), it.upgrade()) else { return };
            let pos = item.position();
            if !me.selection.is_selected(pos) {
                me.selection.select_item(pos, true);
            }
        });
        w.add_controller(click);
    }

    fn signals(self: &Rc<Self>) {
        let activate = {
            let weak = Rc::downgrade(self);
            move |pos: u32| {
                let Some(me) = weak.upgrade() else { return };
                if let Some(o) = me.selection.item(pos) {
                    let e = entry_of(&o);
                    let path = join(&me.cwd.borrow(), &e.name);
                    win().open_entry(&me, path, e);
                }
            }
        };
        let a2 = activate.clone();
        self.view.connect_activate(move |_, pos| activate(pos));
        self.grid.connect_activate(move |_, pos| a2(pos));

        let weak = Rc::downgrade(self);
        self.selection.connect_selection_changed(move |_, _, _| {
            if let Some(me) = weak.upgrade() {
                me.update_status();
                win().pane_selection_changed(&me);
            }
        });

        // filtre : la 1re correspondance est sélectionnée ; Entrée l'ouvre, ↓ descend dans la liste
        let weak = Rc::downgrade(self);
        self.search.connect_search_changed(move |_| {
            if let Some(me) = weak.upgrade() {
                me.filter.changed(gtk::FilterChange::Different);
                if me.selection.n_items() > 0 {
                    me.selection.select_item(0, true);
                    me.scroll_to(0, gtk::ListScrollFlags::NONE);
                } else {
                    me.selection.unselect_all();
                }
                me.update_status();
                win().pane_selection_changed(&me);
            }
        });
        let weak = Rc::downgrade(self);
        self.search.connect_activate(move |_| {
            let Some(me) = weak.upgrade() else { return };
            if let Some((path, e)) = me.selected().into_iter().next() {
                win().open_entry(&me, path, e);
                me.focus_list();
            }
        });
        let weak = Rc::downgrade(self);
        self.search.connect_stop_search(move |_| {
            if let Some(me) = weak.upgrade() {
                me.focus_list();
            }
        });
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(me) = weak.upgrade() else { return glib::Propagation::Proceed };
            // Ctrl+C / Ctrl+X sans texte sélectionné dans le filtre : agissent sur les fichiers
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            if ctrl && matches!(key.to_lower(), gdk::Key::c | gdk::Key::x) && me.search.selection_bounds().is_none() {
                return if win().list_key(&me, key, state) { glib::Propagation::Stop } else { glib::Propagation::Proceed };
            }
            if key == gdk::Key::Down && me.selection.n_items() > 0 {
                me.scroll_to(0, gtk::ListScrollFlags::FOCUS | gtk::ListScrollFlags::SELECT);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        self.search.add_controller(keys);
        self.search_bar.set_key_capture_widget(Some(&self.stack));

        let weak = Rc::downgrade(self);
        self.path_entry.connect_activate(move |e| {
            if let Some(me) = weak.upgrade() {
                win().goto_text(&me, e.text().trim());
                me.focus_list();
            }
        });

        // clavier de la liste et de la grille
        for w in [self.view.clone().upcast::<gtk::Widget>(), self.grid.clone().upcast()] {
            let keys = gtk::EventControllerKey::new();
            keys.set_propagation_phase(gtk::PropagationPhase::Capture);
            let weak = Rc::downgrade(self);
            keys.connect_key_pressed(move |_, key, _, state| {
                let Some(me) = weak.upgrade() else { return glib::Propagation::Proceed };
                if win().list_key(&me, key, state) { glib::Propagation::Stop } else { glib::Propagation::Proceed }
            });
            w.add_controller(keys);
        }

        // menu contextuel
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |_, _, x, y| {
            if let Some(me) = weak.upgrade() {
                me.popup_menu(x, y);
            }
        });
        self.stack.add_controller(click);

        // déposer : copie dans ce dossier
        let drop = gtk::DropTarget::new(glib::Type::INVALID, gdk::DragAction::COPY);
        drop.set_types(&[glib::BoxedAnyObject::static_type(), gdk::FileList::static_type()]);
        let weak = Rc::downgrade(self);
        drop.connect_drop(move |_, value, _, _| {
            let Some(me) = weak.upgrade() else { return false };
            me.root.remove_css_class("drop-here");
            win().drop_into(&me, value)
        });
        let r = self.root.downgrade();
        drop.connect_enter(move |_, _, _| {
            if let Some(r) = r.upgrade() {
                r.add_css_class("drop-here");
            }
            gdk::DragAction::COPY
        });
        let r = self.root.downgrade();
        drop.connect_leave(move |_| {
            if let Some(r) = r.upgrade() {
                r.remove_css_class("drop-here");
            }
        });
        self.stack.add_controller(drop);
    }

    /// Menu contextuel au clavier (touche Menu, Maj+F10) : sous la ligne sélectionnée.
    pub fn popup_menu_here(&self) {
        let pos = (0..self.selection.n_items()).find(|&i| self.selection.is_selected(i)).unwrap_or(0);
        // hauteur de ligne approximative : le menu s'ouvre près de l'élément
        let y = self.stack.height().min(40 + 22 * pos as i32);
        self.popup_menu(60.0, y as f64);
    }

    /// À la fermeture de l'onglet : le menu est rattaché à la liste.
    pub fn dispose(&self) {
        if let Some(m) = self.menu.borrow_mut().take() {
            m.unparent();
        }
        *self.monitor.borrow_mut() = None;
    }

    fn popup_menu(&self, x: f64, y: f64) {
        let mut m = self.menu.borrow_mut();
        let pop = m.get_or_insert_with(|| {
            let p = gtk::PopoverMenu::from_model(Some(&crate::window::context_menu()));
            p.set_parent(&self.stack);
            p.set_has_arrow(false);
            // menu fermé : le clavier revient à la liste (ou à la grille)
            let (v, g, mode) = (self.view.downgrade(), self.grid.downgrade(), self.grid_mode.clone());
            p.connect_closed(move |_| {
                let w: Option<gtk::Widget> = if mode.get() { g.upgrade().map(|x| x.upcast()) } else { v.upgrade().map(|x| x.upcast()) };
                if let Some(w) = w {
                    w.grab_focus();
                }
            });
            p.set_halign(gtk::Align::Start);
            p
        });
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.popup();
    }

    /// Premier élément sélectionné.
    pub fn current(&self) -> Option<(String, Entry)> {
        self.selected().into_iter().next()
    }

    /// Repeint les marques des lignes affichées.
    pub fn redecorate(&self) {
        let bound: Vec<(String, gtk::Box)> =
            self.bound.borrow().iter().flat_map(|(n, v)| v.iter().filter_map(|w| Some((n.clone(), w.upgrade()?)))).collect();
        for (name, b) in bound {
            self.decorate(&b, &name);
        }
    }

    /// Marque d'une ligne : copiée ou coupée (presse-papiers) ; sur la cellule du nom
    /// (toucher à la ligne de GTK pendant sa construction la fait planter).
    fn decorate(&self, b: &gtk::Box, name: &str) {
        let path = join(&self.cwd.borrow(), name);
        let yank = crate::window::try_win().and_then(|w| w.yank_state(&self.fs.borrow(), &path));
        for (cls, on) in [("yank-copy", yank == Some(false)), ("yank-cut", yank == Some(true))] {
            if on {
                b.add_css_class(cls);
            } else {
                b.remove_css_class(cls);
            }
        }
    }

    pub fn set_hidden_changed(&self) {
        self.filter.changed(gtk::FilterChange::Different);
        self.update_status();
    }

    /// Éléments sélectionnés : (chemin complet, entrée).
    pub fn selected(&self) -> Vec<(String, Entry)> {
        let cwd = self.cwd.borrow().clone();
        let bs = self.selection.selection();
        (0..bs.size())
            .filter_map(|k| self.selection.item(bs.nth(k as u32)))
            .map(|o| {
                let e = entry_of(&o);
                (join(&cwd, &e.name), e)
            })
            .collect()
    }

    pub fn update_status(&self) {
        let n = self.selection.n_items();
        let total = self.store.n_items();
        let sel = self.selected();
        let mut s = format!("{n} élément(s)");
        if total > n {
            s.push_str(&format!(" ({} masqué(s))", total - n));
        }
        if !sel.is_empty() {
            let size: u64 = sel.iter().filter(|(_, e)| !e.is_dir).map(|(_, e)| e.size).sum();
            s.push_str(&format!("  ·  {} sélectionné(s), {}", sel.len(), human(size)));
        }

        self.status.set_text(&s);
    }

    pub fn title(&self) -> String {
        let fs = self.fs.borrow();
        let cwd = self.cwd.borrow();
        let name = if *cwd == fs.home { "~" } else if *cwd == "/" { "/" } else { basename(&cwd) };
        if fs.remote || fs.sudo.is_some() { format!("{} : {name}", fs.label) } else { name.to_string() }
    }

    // ---------------------------------------------------------------- chargement

    /// Ouvre `path` ; `select` : nom à sélectionner une fois la liste chargée.
    pub fn load(self: &Rc<Self>, path: String, select: Option<String>) {
        let g = self.generation.get() + 1;
        self.generation.set(g);
        let fs = self.fs.borrow().clone();
        if *self.cwd.borrow() != path {
            self.status.set_text("Chargement…");
        }
        let weak = Rc::downgrade(self);
        bg(
            move || {
                let r = fs.ls(&path);
                (path, r)
            },
            move |(path, r)| {
                let Some(me) = weak.upgrade() else { return };
                if me.generation.get() != g {
                    return;
                }
                match r {
                    Ok(v) => me.apply(path, v, select),
                    Err(e) => win().load_failed(&me, path, e),
                }
            },
        );
    }

    pub fn refresh(self: &Rc<Self>) {
        let cwd = self.cwd.borrow().clone();
        if !cwd.is_empty() {
            self.load(cwd, None);
        }
    }

    pub fn navigate(self: &Rc<Self>, path: String) {
        let cwd = self.cwd.borrow().clone();
        if !cwd.is_empty() && cwd != path {
            self.back.borrow_mut().push(cwd);
            self.fwd.borrow_mut().clear();
        }
        self.load(path, None);
    }

    /// Va dans `path` et met le curseur sur `name`.
    pub fn navigate_select(self: &Rc<Self>, path: String, name: String) {
        let cwd = self.cwd.borrow().clone();
        if !cwd.is_empty() && cwd != path {
            self.back.borrow_mut().push(cwd);
            self.fwd.borrow_mut().clear();
        }
        self.load(path, Some(name));
    }

    pub fn history(&self) -> (bool, bool) {
        (!self.back.borrow().is_empty(), !self.fwd.borrow().is_empty())
    }

    pub fn go_back(self: &Rc<Self>) {
        let Some(p) = self.back.borrow_mut().pop() else { return };
        self.fwd.borrow_mut().push(self.cwd.borrow().clone());
        self.load(p, None);
    }

    pub fn go_forward(self: &Rc<Self>) {
        let Some(p) = self.fwd.borrow_mut().pop() else { return };
        self.back.borrow_mut().push(self.cwd.borrow().clone());
        self.load(p, None);
    }

    pub fn go_up(self: &Rc<Self>) {
        let cwd = self.cwd.borrow().clone();
        if cwd == "/" || cwd.is_empty() {
            return;
        }
        self.back.borrow_mut().push(cwd.clone());
        self.fwd.borrow_mut().clear();
        self.load(dirname(&cwd), Some(basename(&cwd).to_string()));
    }

    fn apply(self: &Rc<Self>, path: String, entries: Vec<Entry>, select: Option<String>) {
        let same_dir = *self.cwd.borrow() == path;
        if same_dir {
            // une ligne qui avait le focus peut disparaître : on le rend à la liste
            let had_focus = self.focus_in_list();
            self.update_in_place(entries);
            if had_focus && !self.focus_in_list() {
                self.focus_list();
            }
        } else {
            *self.cwd.borrow_mut() = path.clone();
            self.search.set_text("");
            self.search_bar.set_search_mode(false);
            let objs: Vec<glib::BoxedAnyObject> = entries.into_iter().map(boxed).collect();
            self.store.splice(0, self.store.n_items(), &objs);
            self.selection.unselect_all();
            self.watch(&path);
            let fs = self.fs.borrow().clone();
            self.path_entry.set_text(&fs.pretty(&path));
            if self.selection.n_items() > 0 {
                self.scroll_to(0, gtk::ListScrollFlags::FOCUS);
            }
        }
        // élément à sélectionner : après coup, quand la liste a recréé ses lignes
        // (le faire tout de suite peut planter GTK)
        if let Some(name) = select {
            let weak = Rc::downgrade(self);
            glib::idle_add_local_once(move || {
                let Some(me) = weak.upgrade() else { return };
                let pos = (0..me.selection.n_items()).find(|&i| me.selection.item(i).map(|o| entry_of(&o).name == name).unwrap_or(false));
                if let Some(pos) = pos {
                    me.scroll_to(pos, gtk::ListScrollFlags::FOCUS | gtk::ListScrollFlags::SELECT);
                }
            });
        }
        self.update_status();
        let w = win();
        w.pane_loaded(self);
        w.pane_selection_changed(self);
    }

    fn focus_in_list(&self) -> bool {
        self.stack.root().and_then(|r| r.focus()).map(|f| f.is_ancestor(&self.stack)).unwrap_or(false)
    }

    /// Rafraîchit sans perdre la position : retire, remplace et ajoute ce qui a changé.
    fn update_in_place(&self, entries: Vec<Entry>) {
        let mut new: HashMap<String, Entry> = entries.into_iter().map(|e| (e.name.clone(), e)).collect();
        let mut i = self.store.n_items();
        while i > 0 {
            i -= 1;
            let Some(o) = self.store.item(i) else { continue };
            let old = entry_of(&o);
            match new.remove(&old.name) {
                None => self.store.remove(i),
                Some(n) if n != old => self.store.splice(i, 1, &[boxed(n)]),
                Some(_) => {}
            }
        }
        let rest: Vec<glib::BoxedAnyObject> = new.into_values().map(boxed).collect();
        if !rest.is_empty() {
            self.store.extend_from_slice(&rest);
        }
    }

    /// Dossier local : rafraîchi tout seul quand il change.
    fn watch(self: &Rc<Self>, path: &str) {
        *self.monitor.borrow_mut() = None;
        if self.fs.borrow().remote {
            return;
        }
        let Ok(mon) = gio::File::for_path(path).monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) else { return };
        let weak = Rc::downgrade(self);
        let pending = Rc::new(Cell::new(false));
        mon.connect_changed(move |_, _, _, _| {
            if pending.replace(true) {
                return;
            }
            let (weak, pending) = (weak.clone(), pending.clone());
            glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
                pending.set(false);
                if let Some(me) = weak.upgrade() {
                    me.fs.borrow().clear_cache();
                    me.refresh();
                }
            });
        });
        *self.monitor.borrow_mut() = Some(mon);
    }

    pub fn focus_list(&self) {
        self.active_view().grab_focus();
    }
}
