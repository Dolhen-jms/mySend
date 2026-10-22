//! « Envoyer à un appareil Apple » : le radar du panneau quickshell (omdrop), en fenêtre.
//! Un point par appareil entendu, qui pulse au passage du balayage ; un clic sur un point
//! (ou sur sa ligne) envoie la sélection. Le dernier appareil utilisé est proposé d'emblée.

use crate::util::{bg, escape, on_main};
use crate::window::{win, Win};
use adw::prelude::*;
use gtk::glib;
use mysend::airdrop::{self, Peer, Target};
use mysend::fs::{Entry, FsRef};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::process::Child;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    /// Lecture de l'état d'omdrop
    Check,
    /// `omdrop on` en cours, ou en attente de la radio
    Starting,
    /// La radio annonce : on cherche les appareils
    Scanning,
    Failed,
}

struct Radar {
    dialog: adw::Dialog,
    area: gtk::DrawingArea,
    status: gtk::Label,
    hint: gtk::Label,
    list: gtk::ListBox,
    phase: Cell<Phase>,
    peers: RefCell<Vec<Peer>>,
    names: RefCell<HashMap<String, String>>,
    /// adresses affichées dans la liste, dans l'ordre
    rows: RefCell<Vec<String>>,
    sweep: Cell<f64>,
    since: Cell<Instant>,
    dots: Cell<usize>,
    names_proc: RefCell<Option<Child>>,
    waker: RefCell<Option<Child>>,
    alive: Cell<bool>,
    /// AirDrop était éteint : c'est nous qui l'avons allumé (on l'éteindra).
    turned_on: Cell<bool>,
    /// Un envoi est parti (c'est lui qui éteindra AirDrop).
    sent: Cell<bool>,
    fs: FsRef,
    items: Vec<(String, Entry)>,
}

thread_local! {
    static RADAR: RefCell<Weak<Radar>> = RefCell::new(Weak::new());
    /// La fenêtre ouverte garde son radar en vie (relâché à la fermeture).
    static OPEN: RefCell<Option<Rc<Radar>>> = const { RefCell::new(None) };
}

fn radar() -> Option<Rc<Radar>> {
    RADAR.with(|r| r.borrow().upgrade())
}

fn rgb(hex: &str) -> (f64, f64, f64) {
    let (r, g, b) = mysend::theme::rgb(hex);
    (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
}

impl Win {
    /// Envoie la sélection de l'onglet courant à un appareil Apple proche.
    pub fn airdrop(self: &Rc<Self>) {
        let Some(p) = self.current() else { return };
        let items = p.selected();
        if items.is_empty() {
            self.toast("Sélectionne d'abord ce qu'il faut envoyer.");
            return;
        }
        self.airdrop_items(p.fs.borrow().clone(), items);
    }

    pub fn airdrop_items(self: &Rc<Self>, fs: FsRef, items: Vec<(String, Entry)>) {
        if !airdrop::available() {
            self.error(airdrop::unavailable_reason());
            return;
        }
        if airdrop::native() {
            // macOS : panneau AirDrop du système, qui choisit l'appareil
            self.toast("AirDrop : choisis l'appareil dans la fenêtre du Mac.");
            self.enqueue(airdrop::native_task(fs, items));
            return;
        }
        Radar::open(self, fs, items);
    }
}

impl Radar {
    fn open(w: &Rc<Win>, fs: FsRef, items: Vec<(String, Entry)>) {
        let d = adw::Dialog::new();
        d.set_title("Envoyer à un appareil Apple");
        d.set_content_width(460);
        let tb = adw::ToolbarView::new();
        tb.add_top_bar(&adw::HeaderBar::new());
        let col = gtk::Box::new(gtk::Orientation::Vertical, 10);
        col.set_margin_top(4);
        col.set_margin_bottom(16);
        col.set_margin_start(16);
        col.set_margin_end(16);

        let what = if items.len() == 1 { items[0].1.name.clone() } else { format!("{} éléments", items.len()) };
        let files = gtk::Label::new(None);
        files.set_markup(&format!("<b>{}</b>{}", escape(&what), if items.iter().any(|(_, e)| e.is_dir) { "  <small>(dossiers envoyés en .zip)</small>" } else { "" }));
        files.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        col.append(&files);

        let status = gtk::Label::new(Some("Lecture de l'état d'AirDrop"));
        status.add_css_class("muted");
        col.append(&status);

        let area = gtk::DrawingArea::new();
        area.set_content_width(280);
        area.set_content_height(280);
        area.set_halign(gtk::Align::Center);
        col.append(&area);

        let hint = gtk::Label::new(None);
        hint.add_css_class("muted");
        hint.set_wrap(true);
        hint.set_justify(gtk::Justification::Center);
        hint.set_visible(false);
        col.append(&hint);

        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        col.append(&list);

        // le dernier appareil, et un nom à taper
        let more = gtk::ListBox::new();
        more.add_css_class("boxed-list");
        more.set_selection_mode(gtk::SelectionMode::None);
        if let Some(last) = airdrop::last_device() {
            let r = adw::ActionRow::new();
            r.set_title(&escape(&last));
            r.set_subtitle("dernier appareil utilisé : le chercher par son nom");
            r.add_prefix(&gtk::Image::from_icon_name("phone-symbolic"));
            r.set_activatable(true);
            let last2 = last.clone();
            r.connect_activated(move |_| {
                if let Some(rd) = radar() {
                    rd.send(Target::Name(last2.clone()));
                }
            });
            more.append(&r);
        }
        let entry = adw::EntryRow::new();
        entry.set_title("… ou le nom d'un appareil");
        entry.set_show_apply_button(true);
        entry.connect_apply(|e| {
            let n = e.text().trim().to_string();
            if let (false, Some(rd)) = (n.is_empty(), radar()) {
                rd.send(Target::Name(n));
            }
        });
        more.append(&entry);
        col.append(&more);

        tb.set_content(Some(&col));
        d.set_child(Some(&tb));

        let rd = Rc::new(Radar {
            dialog: d.clone(),
            area: area.clone(),
            status,
            hint,
            list: list.clone(),
            phase: Cell::new(Phase::Check),
            peers: RefCell::default(),
            names: RefCell::default(),
            rows: RefCell::default(),
            sweep: Cell::new(0.0),
            since: Cell::new(Instant::now()),
            dots: Cell::new(0),
            names_proc: RefCell::new(None),
            waker: RefCell::new(None),
            alive: Cell::new(true),
            turned_on: Cell::new(false),
            sent: Cell::new(false),
            fs,
            items,
        });
        RADAR.with(|r| *r.borrow_mut() = Rc::downgrade(&rd));
        OPEN.with(|o| *o.borrow_mut() = Some(rd.clone()));

        let weak = Rc::downgrade(&rd);
        area.set_draw_func(move |_, cr, wd, ht| {
            if let Some(rd) = weak.upgrade() {
                rd.draw(cr, wd as f64, ht as f64);
            }
        });
        let click = gtk::GestureClick::new();
        click.connect_pressed(|g, _, x, y| {
            let Some(rd) = radar() else { return };
            let (w, h) = (g.widget().map(|w| w.width() as f64).unwrap_or(0.0), g.widget().map(|w| w.height() as f64).unwrap_or(0.0));
            if let Some(p) = rd.peer_at(x, y, w, h) {
                rd.send(Target::Mac { name: rd.name_of(&p), mac: p.mac });
            }
        });
        area.add_controller(click);
        list.connect_row_activated(|_, row| {
            let Some(rd) = radar() else { return };
            let mac = rd.rows.borrow().get(row.index() as usize).cloned();
            if let Some(mac) = mac {
                rd.send(Target::Mac { name: rd.names.borrow().get(&mac).cloned().unwrap_or_default(), mac });
            }
        });
        d.connect_closed(|_| {
            if let Some(rd) = radar() {
                rd.shutdown();
            }
            OPEN.with(|o| o.borrow_mut().take());
        });

        // animation (balayage en 4 s) et suivi de l'état, toutes les 33 ms
        let weak = Rc::downgrade(&rd);
        glib::timeout_add_local(Duration::from_millis(33), move || {
            let Some(rd) = weak.upgrade() else { return glib::ControlFlow::Break };
            if !rd.alive.get() {
                return glib::ControlFlow::Break;
            }
            if rd.phase.get() == Phase::Scanning {
                rd.sweep.set((rd.sweep.get() + 360.0 * 0.033 / 4.0) % 360.0);
            }
            rd.area.queue_draw();
            glib::ControlFlow::Continue
        });
        // points d'attente, conseils, relevés : toutes les secondes
        let weak = Rc::downgrade(&rd);
        glib::timeout_add_local(Duration::from_millis(1000), move || {
            let Some(rd) = weak.upgrade() else { return glib::ControlFlow::Break };
            if !rd.alive.get() {
                return glib::ControlFlow::Break;
            }
            rd.tick();
            glib::ControlFlow::Continue
        });
        d.present(Some(&w.window));
        rd.check();
    }

    /// Lit l'état d'omdrop ; allume AirDrop s'il est éteint.
    fn check(&self) {
        bg(airdrop::status, |s| {
            let Some(rd) = radar() else { return };
            match s {
                None => rd.fail("omdrop ne répond pas (omdrop doctor pour le diagnostic)."),
                Some(s) if s.receiving && s.visible => rd.start_scanning(),
                Some(s) if s.receiving => rd.phase.set(Phase::Starting),
                Some(_) => {
                    rd.phase.set(Phase::Starting);
                    rd.turned_on.set(true);
                    bg(airdrop::turn_on, |r| {
                        if let (Err(e), Some(rd)) = (r, radar()) {
                            rd.fail(&format!("AirDrop ne s'allume pas : {e}"));
                        }
                    });
                }
            }
        });
    }

    fn fail(&self, msg: &str) {
        self.phase.set(Phase::Failed);
        self.status.set_text(msg);
    }

    fn start_scanning(&self) {
        if self.phase.replace(Phase::Scanning) != Phase::Scanning {
            self.since.set(Instant::now());
            self.spawn_names();
        }
    }

    /// La recherche des noms tourne en continu ; le réveilleur prend le relais entre deux.
    fn spawn_names(&self) {
        if !self.alive.get() {
            return;
        }
        if let Some(mut w) = self.waker.borrow_mut().take() {
            airdrop::stop(&mut w);
        }
        let started = Instant::now();
        let child = airdrop::spawn_names(|mac, name| {
            on_main(move || {
                if let (Some(rd), false) = (radar(), name.is_empty()) {
                    rd.names.borrow_mut().insert(mac, name);
                    rd.rebuild_list();
                }
            })
        });
        let Some(child) = child else { return };
        let pid = child.id();
        *self.names_proc.borrow_mut() = Some(child);
        // fin du processus : relance (vite s'il a tourné, sinon après 10 s), réveilleur entre-temps
        std::thread::spawn(move || {
            let mut st = 0;
            unsafe { libc::waitpid(pid as i32, &mut st, 0) };
            let ran = started.elapsed();
            on_main(move || {
                let Some(rd) = radar() else { return };
                rd.names_proc.borrow_mut().take();
                if !rd.alive.get() {
                    return;
                }
                *rd.waker.borrow_mut() = airdrop::spawn_waker();
                let delay = if ran < Duration::from_secs(5) { 10_000 } else { 250 };
                glib::timeout_add_local_once(Duration::from_millis(delay), || {
                    if let Some(rd) = radar() {
                        rd.spawn_names();
                    }
                });
            });
        });
    }

    fn tick(&self) {
        self.dots.set(self.dots.get() % 3 + 1);
        let dots = ".".repeat(self.dots.get());
        match self.phase.get() {
            Phase::Check => self.status.set_text(&format!("Lecture de l'état d'AirDrop{dots}")),
            Phase::Starting => {
                self.status.set_text(&format!("Allumage d'AirDrop{dots}"));
                bg(airdrop::status, |s| {
                    if let (Some(rd), Some(s)) = (radar(), s) {
                        if s.receiving && s.visible {
                            rd.start_scanning();
                        }
                    }
                });
            }
            Phase::Scanning => {
                let n = self.peers.borrow().len();
                self.status.set_text(&match n {
                    0 => format!("Recherche des appareils{dots}"),
                    1 => "1 appareil : clique dessus pour envoyer".to_string(),
                    n => format!("{n} appareils : clique sur l'un d'eux pour envoyer"),
                });
                // rien au bout de 15 s : la cause est presque toujours le réglage du téléphone
                let quiet = self.since.get().elapsed() > Duration::from_secs(15) && n == 0;
                self.hint.set_visible(quiet);
                if quiet {
                    self.hint.set_text("Rien n'arrive ? Sur l'iPhone : écran allumé et déverrouillé, AirDrop sur « Tout le monde pendant 10 minutes ».");
                }
                // relevé des appareils toutes les 2 s
                if self.dots.get() % 2 == 0 {
                    bg(airdrop::peers, |v| {
                        if let Some(rd) = radar() {
                            *rd.peers.borrow_mut() = v;
                            rd.rebuild_list();
                        }
                    });
                }
            }
            Phase::Failed => {}
        }
    }

    fn name_of(&self, p: &Peer) -> String {
        if !p.name.is_empty() {
            return p.name.clone();
        }
        self.names.borrow().get(&p.mac).cloned().unwrap_or_default()
    }

    fn rebuild_list(&self) {
        let peers = self.peers.borrow().clone();
        let macs: Vec<String> = peers.iter().map(|p| p.mac.clone()).collect();
        let labels: Vec<(String, String)> = peers.iter().map(|p| (self.name_of(p), signal(p.rssi))).collect();
        // ne reconstruit que si quelque chose a changé (la liste garde sa sélection sinon)
        let same = *self.rows.borrow() == macs
            && (0..macs.len()).all(|i| self.list.row_at_index(i as i32).and_then(|r| r.child()).and_then(|c| c.downcast::<adw::ActionRow>().ok()).map(|r| r.title() == escape(&display(&labels[i].0))).unwrap_or(false));
        if same {
            return;
        }
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        for (name, sig) in &labels {
            let r = adw::ActionRow::new();
            r.set_title(&escape(&display(name)));
            r.set_subtitle(&escape(sig));
            r.add_prefix(&gtk::Image::from_icon_name(if name.is_empty() { "network-wireless-symbolic" } else { "phone-symbolic" }));
            r.add_suffix(&gtk::Image::from_icon_name("document-send-symbolic"));
            r.set_activatable(true);
            self.list.append(&r);
        }
        *self.rows.borrow_mut() = macs;
    }

    /// Point du radar sous (x, y), s'il y en a un.
    fn peer_at(&self, x: f64, y: f64, w: f64, h: f64) -> Option<Peer> {
        let (cx, cy, r) = (w / 2.0, h / 2.0, w.min(h) / 2.0 - 10.0);
        self.peers
            .borrow()
            .iter()
            .map(|p| {
                let (a, d) = airdrop::radar_position(p);
                let (px, py) = (cx + a.sin() * d * r, cy - a.cos() * d * r);
                (((px - x).powi(2) + (py - y).powi(2)).sqrt(), p.clone())
            })
            .filter(|(dist, _)| *dist < 22.0)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, p)| p)
    }

    fn draw(&self, cr: &gtk::cairo::Context, w: f64, h: f64) {
        let pal = crate::theme::current();
        let (pr, pg, pb) = rgb(&pal.primary);
        let (mr, mg, mb) = rgb(&pal.muted);
        let (fr, fg, fb) = rgb(&pal.fg);
        let (cx, cy, r) = (w / 2.0, h / 2.0, w.min(h) / 2.0 - 10.0);
        let scanning = self.phase.get() == Phase::Scanning;
        // cercles et axes
        cr.set_line_width(1.0);
        for k in 1..=3 {
            cr.set_source_rgba(mr, mg, mb, 0.35);
            cr.arc(cx, cy, r * k as f64 / 3.0, 0.0, std::f64::consts::TAU);
            let _ = cr.stroke();
        }
        cr.set_source_rgba(mr, mg, mb, 0.18);
        cr.move_to(cx - r, cy);
        cr.line_to(cx + r, cy);
        cr.move_to(cx, cy - r);
        cr.line_to(cx, cy + r);
        let _ = cr.stroke();
        // balayage : un éventail qui s'estompe derrière l'aiguille
        let sweep = self.sweep.get().to_radians();
        if scanning {
            for i in 0..24 {
                let a1 = sweep - (i as f64) * 0.035;
                let a0 = a1 - 0.035;
                cr.set_source_rgba(pr, pg, pb, 0.22 * (1.0 - i as f64 / 24.0));
                cr.move_to(cx, cy);
                cr.arc(cx, cy, r, a0 - std::f64::consts::FRAC_PI_2, a1 - std::f64::consts::FRAC_PI_2);
                cr.close_path();
                let _ = cr.fill();
            }
            cr.set_source_rgba(pr, pg, pb, 0.9);
            cr.set_line_width(2.0);
            cr.move_to(cx, cy);
            cr.line_to(cx + sweep.sin() * r, cy - sweep.cos() * r);
            let _ = cr.stroke();
        }
        // cette machine, au centre
        cr.set_source_rgb(fr, fg, fb);
        cr.arc(cx, cy, 4.0, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        // les appareils : un point qui pulse quand l'aiguille passe dessus
        cr.select_font_face("MonoLisa", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
        cr.set_font_size(12.0);
        for p in self.peers.borrow().iter() {
            let (a, d) = airdrop::radar_position(p);
            let (px, py) = (cx + a.sin() * d * r, cy - a.cos() * d * r);
            let behind = ((sweep - a).rem_euclid(std::f64::consts::TAU)) / std::f64::consts::TAU;
            let glow = (1.0 - behind * 3.0).max(0.0);
            cr.set_source_rgba(pr, pg, pb, 0.25 * glow);
            cr.arc(px, py, 7.0 + 10.0 * glow, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
            cr.set_source_rgb(pr, pg, pb);
            cr.arc(px, py, 6.0, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
            let name = self.name_of(p);
            if !name.is_empty() {
                cr.set_source_rgb(fr, fg, fb);
                let ext = cr.text_extents(&name).ok();
                let tw = ext.map(|e| e.width()).unwrap_or(0.0);
                let tx = (px - tw / 2.0).clamp(2.0, w - tw - 2.0);
                cr.move_to(tx, py + 20.0);
                let _ = cr.show_text(&name);
            }
        }
    }

    fn send(&self, target: Target) {
        let items = self.items.clone();
        let fs = self.fs.clone();
        self.sent.set(true);
        let turn_off = self.turned_on.get();
        self.dialog.close();
        let w = win();
        w.toast(&format!("Envoi vers {} : accepte sur l'appareil.", target.label()));
        w.enqueue(airdrop::send_task(fs, items, target, turn_off));
    }

    fn shutdown(&self) {
        self.alive.set(false);
        if let Some(mut c) = self.names_proc.borrow_mut().take() {
            airdrop::stop(&mut c);
        }
        if let Some(mut c) = self.waker.borrow_mut().take() {
            airdrop::stop(&mut c);
        }
        // fermé sans rien envoyer : AirDrop retourne à son état d'avant
        if self.turned_on.get() && !self.sent.get() {
            bg(airdrop::turn_off, |_| {});
        }
    }
}

fn display(name: &str) -> String {
    if name.is_empty() { "Appareil sans nom (demande en cours)".into() } else { name.to_string() }
}

/// Force du signal en barres.
fn signal(rssi: Option<i64>) -> String {
    match rssi {
        Some(r) if r > -55 => "signal ▂▄▆█ (tout près)".into(),
        Some(r) if r > -67 => "signal ▂▄▆".into(),
        Some(r) if r > -80 => "signal ▂▄".into(),
        Some(_) => "signal ▂ (loin)".into(),
        None => "signal inconnu".into(),
    }
}
