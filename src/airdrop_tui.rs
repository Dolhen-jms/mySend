//! AirDrop dans l'interface terminal (touche A, ou :airdrop) : un radar en caractères —
//! cercles, aiguille qui tourne et laisse une traînée, appareils numérotés à leur place —
//! et la liste à côté. 1-9 envoie à l'appareil, l au dernier utilisé, n à un nom tapé.

use crate::airdrop::{self, Peer, Target};
use crate::app::{App, Severity};
use crate::fs::{Entry, FsRef};
use crate::modals::ModalKind;
use crate::style::*;
use crate::ui::{centered, dialog};
use crate::util::truncate;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Check,
    Starting,
    Scanning,
}

#[derive(Default)]
struct State {
    phase: Option<Phase>,
    peers: Vec<Peer>,
    names: HashMap<String, String>,
    error: Option<String>,
    scanning_since: Option<Instant>,
    /// AirDrop était éteint : c'est nous qui l'avons allumé (on l'éteindra).
    turned_on: bool,
}

pub struct AirModal {
    st: Arc<Mutex<State>>,
    alive: Arc<AtomicBool>,
    opened: Instant,
    fs: FsRef,
    items: Vec<(String, Entry)>,
    last: Option<String>,
    /// Un envoi est parti (c'est lui qui éteindra AirDrop).
    sent: bool,
}

impl Drop for AirModal {
    fn drop(&mut self) {
        // fenêtre fermée : le fil de recherche s'arrête (et arrête ses processus)
        self.alive.store(false, Ordering::Relaxed);
        // fermée sans rien envoyer : AirDrop retourne à son état d'avant
        if self.st.lock().unwrap().turned_on && !self.sent {
            std::thread::spawn(|| airdrop::turn_off());
        }
    }
}

/// Le fil de recherche : état, allumage, relevés toutes les 2 s, noms en continu,
/// réveilleur Bluetooth entre deux recherches de noms.
fn worker(st: Arc<Mutex<State>>, alive: Arc<AtomicBool>) {
    let set = |f: &dyn Fn(&mut State)| f(&mut st.lock().unwrap());
    let on = || alive.load(Ordering::Relaxed);
    set(&|s| s.phase = Some(Phase::Check));
    match airdrop::status() {
        None => return set(&|s| s.error = Some("omdrop ne répond pas (omdrop doctor pour le diagnostic).".into())),
        Some(s) if !s.receiving => {
            set(&|s| {
                s.phase = Some(Phase::Starting);
                s.turned_on = true;
            });
            if let Err(e) = airdrop::turn_on() {
                let msg = format!("AirDrop ne s'allume pas : {e}");
                return set(&|s| s.error = Some(msg.clone()));
            }
        }
        Some(_) => set(&|s| s.phase = Some(Phase::Starting)),
    }
    while on() {
        if airdrop::status().map(|s| s.receiving && s.visible).unwrap_or(false) {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    set(&|s| {
        s.phase = Some(Phase::Scanning);
        s.scanning_since = Some(Instant::now());
    });
    let mut names: Option<(std::process::Child, Instant)> = None;
    let mut waker: Option<std::process::Child> = None;
    let mut next_names = Instant::now();
    while on() {
        let peers = airdrop::peers();
        set(&|s| s.peers = peers.clone());
        for _ in 0..20 {
            if !on() {
                break;
            }
            // fin de la recherche des noms : réveilleur, puis relance (vite si elle a tourné)
            if let Some((c, t)) = &mut names {
                if c.try_wait().ok().flatten().is_some() {
                    next_names = Instant::now() + if t.elapsed() < Duration::from_secs(5) { Duration::from_secs(10) } else { Duration::from_millis(250) };
                    names = None;
                    waker = airdrop::spawn_waker();
                }
            }
            if names.is_none() && Instant::now() >= next_names {
                if let Some(mut w) = waker.take() {
                    airdrop::stop(&mut w);
                }
                let st2 = st.clone();
                names = airdrop::spawn_names(move |mac, name| {
                    if !name.is_empty() {
                        st2.lock().unwrap().names.insert(mac, name);
                    }
                })
                .map(|c| (c, Instant::now()));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if let Some((mut c, _)) = names {
        airdrop::stop(&mut c);
    }
    if let Some(mut w) = waker {
        airdrop::stop(&mut w);
    }
}

impl App {
    /// A : envoyer la sélection (ou l'élément sous le curseur) à un appareil Apple proche.
    pub fn airdrop(&mut self) {
        if !airdrop::available() {
            self.error(airdrop::unavailable_reason());
            return;
        }
        let fs = self.tab().fs.clone();
        let items = self.tab_mut().targets();
        if items.is_empty() {
            return;
        }
        let st = Arc::new(Mutex::new(State::default()));
        let alive = Arc::new(AtomicBool::new(true));
        let (s2, a2) = (st.clone(), alive.clone());
        std::thread::spawn(move || worker(s2, a2));
        let m = AirModal { st, alive, opened: Instant::now(), fs, items, last: airdrop::last_device(), sent: false };
        self.push_modal(ModalKind::AirDrop(m));
    }
}

fn name_of(p: &Peer, names: &HashMap<String, String>) -> String {
    if !p.name.is_empty() { p.name.clone() } else { names.get(&p.mac).cloned().unwrap_or_default() }
}

fn send(app: &mut App, m: &mut AirModal, target: Target) {
    m.sent = true;
    let turn_off = m.st.lock().unwrap().turned_on;
    app.notify(&format!("AirDrop vers {} : accepte sur l'appareil.", target.label()), Severity::Info, 4.0);
    app.enqueue(airdrop::send_task(m.fs.clone(), m.items.clone(), target, turn_off));
}

/// Touche dans la fenêtre ; faux pour la fermer.
pub fn key(app: &mut App, m: &mut AirModal, name: &str) -> bool {
    match name {
        "escape" | "q" => false,
        "l" => match m.last.clone() {
            Some(n) => {
                send(app, m, Target::Name(n));
                false
            }
            None => true,
        },
        "n" => {
            m.sent = true;
            let (fs, items) = (m.fs.clone(), m.items.clone());
            let turn_off = m.st.lock().unwrap().turned_on;
            app.prompt("Nom de l'appareil (une partie suffit) :", "", None, move |app, v| {
                let Some(n) = v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else {
                    // nom abandonné : AirDrop retourne à son état d'avant
                    if turn_off {
                        std::thread::spawn(|| airdrop::turn_off());
                    }
                    return;
                };
                app.notify(&format!("AirDrop vers {n} : accepte sur l'appareil."), Severity::Info, 4.0);
                app.enqueue(airdrop::send_task(fs, items, Target::Name(n), turn_off));
            });
            false
        }
        d if d.len() == 1 && ('1'..='9').contains(&d.chars().next().unwrap()) => {
            let i = d.parse::<usize>().unwrap() - 1;
            let st = m.st.lock().unwrap();
            let Some(p) = st.peers.get(i).cloned() else { return true };
            let name = name_of(&p, &st.names);
            drop(st);
            send(app, m, Target::Mac { mac: p.mac, name });
            false
        }
        _ => true,
    }
}

fn bars(rssi: Option<i64>) -> &'static str {
    match rssi {
        Some(r) if r > -55 => "▂▄▆█",
        Some(r) if r > -67 => "▂▄▆ ",
        Some(r) if r > -80 => "▂▄  ",
        Some(_) => "▂   ",
        None => "?   ",
    }
}

pub fn render(f: &mut Frame, m: &AirModal) {
    let full = f.area();
    let w = 84.min(full.width);
    let h = 22.min(full.height);
    let inner = dialog(f, centered(full, w, h), Some("AirDrop · envoyer à un appareil Apple"));
    let st = m.st.lock().unwrap();
    let dots = ".".repeat((m.opened.elapsed().as_millis() / 400 % 3 + 1) as usize);
    let accent = accent();

    // en-tête : quoi, et où en est la radio
    let what = if m.items.len() == 1 { m.items[0].1.name.clone() } else { format!("{} éléments", m.items.len()) };
    let mut head = vec![Line::from(vec![
        Span::styled(truncate(&what, inner.width as usize - 30), b()),
        Span::styled(if m.items.iter().any(|(_, e)| e.is_dir) { "  (dossiers envoyés en .zip)" } else { "" }, dim()),
    ])];
    let status = if let Some(e) = &st.error {
        Span::styled(e.clone(), fg(RED))
    } else {
        match st.phase {
            None | Some(Phase::Check) => Span::styled(format!("Lecture de l'état d'AirDrop{dots}"), dim()),
            Some(Phase::Starting) => Span::styled(format!("Allumage d'AirDrop{dots}"), dim()),
            Some(Phase::Scanning) if st.peers.is_empty() => Span::styled(format!("Recherche des appareils{dots}"), dim()),
            Some(Phase::Scanning) => Span::styled(format!("{} appareil(s) : leur numéro pour envoyer", st.peers.len()), fg(GREEN)),
        }
    };
    head.push(Line::from(status));
    f.render_widget(Paragraph::new(head), Rect { height: 2, ..inner });

    // le radar : 31 × 15 cases (une case est deux fois plus haute que large)
    let (rw, rh) = (31i32, 15i32);
    let (cx, cy) = (rw / 2, rh / 2);
    let scanning = st.phase == Some(Phase::Scanning) && st.error.is_none();
    let sweep = (m.opened.elapsed().as_secs_f64() * std::f64::consts::TAU / 4.0) % std::f64::consts::TAU;
    let mut grid: Vec<Vec<(String, Style)>> = vec![vec![(" ".to_string(), Style::default()); rw as usize]; rh as usize];
    for y in 0..rh {
        for x in 0..rw {
            let (dx, dy) = ((x - cx) as f64 / cx as f64, (y - cy) as f64 / cy as f64);
            let d = (dx * dx + dy * dy).sqrt();
            if d > 1.04 {
                continue;
            }
            let a = dx.atan2(-dy).rem_euclid(std::f64::consts::TAU);
            let behind = (sweep - a).rem_euclid(std::f64::consts::TAU);
            let cell = &mut grid[y as usize][x as usize];
            if [1.0 / 3.0, 2.0 / 3.0, 1.0].iter().any(|r| (d - r).abs() < 0.05) {
                *cell = ("·".into(), fg(BRIGHT_BLACK));
            }
            if scanning && d <= 1.0 {
                if behind < 0.12 {
                    *cell = ("•".into(), Style::default().fg(accent).add_modifier(Modifier::BOLD));
                } else if behind < 0.45 {
                    *cell = ("░".into(), fg(accent));
                }
            }
        }
    }
    grid[cy as usize][cx as usize] = ("◆".into(), b());
    for (i, p) in st.peers.iter().take(9).enumerate() {
        let (a, d) = airdrop::radar_position(p);
        let x = cx + (a.sin() * d * cx as f64).round() as i32;
        let y = cy - (a.cos() * d * cy as f64).round() as i32;
        if (0..rw).contains(&x) && (0..rh).contains(&y) {
            let behind = (sweep - a).rem_euclid(std::f64::consts::TAU);
            let mut st = Style::default().fg(accent).add_modifier(Modifier::BOLD);
            if scanning && behind < 0.6 {
                st = st.add_modifier(Modifier::REVERSED);
            }
            grid[y as usize][x as usize] = ((i + 1).to_string(), st);
        }
    }
    let radar: Vec<Line> = grid.into_iter().map(|row| Line::from(row.into_iter().map(|(c, s)| Span::styled(c, s)).collect::<Vec<_>>())).collect();
    let body = Rect { y: inner.y + 3, height: inner.height.saturating_sub(5), ..inner };
    f.render_widget(Paragraph::new(radar), Rect { width: rw as u16 + 2, ..body });

    // la liste
    let lx = body.x + rw as u16 + 4;
    let lw = body.width.saturating_sub(rw as u16 + 4) as usize;
    let mut list: Vec<Line> = Vec::new();
    for (i, p) in st.peers.iter().take(9).enumerate() {
        let n = name_of(p, &st.names);
        let label = if n.is_empty() { "sans nom (demande…)".to_string() } else { n };
        list.push(Line::from(vec![
            Span::styled(format!("{}  ", i + 1), bold(YELLOW)),
            Span::styled(format!("{} ", bars(p.rssi)), fg(GREEN)),
            Span::raw(truncate(&label, lw.saturating_sub(9))),
        ]));
    }
    if st.peers.is_empty() && scanning && st.scanning_since.map(|t| t.elapsed() > Duration::from_secs(15)).unwrap_or(false) {
        for l in ["Rien n'arrive ? Sur l'iPhone :", "écran allumé, déverrouillé,", "AirDrop sur « Tout le monde", "pendant 10 minutes »."] {
            list.push(Line::from(Span::styled(l, dim_italic())));
        }
    }
    if let Some(last) = &m.last {
        list.push(Line::from(""));
        list.push(Line::from(vec![Span::styled("l  ", bold(YELLOW)), Span::raw(truncate(&format!("dernier : {last}"), lw.saturating_sub(3)))]));
    }
    list.push(Line::from(vec![Span::styled("n  ", bold(YELLOW)), Span::raw("taper un nom d'appareil")]));
    f.render_widget(Paragraph::new(list), Rect { x: lx, width: lw as u16, ..body });

    let hint = Line::from(Span::styled("1-9 envoyer · l dernier · n nom · Échap fermer", dim()));
    f.render_widget(Paragraph::new(hint), Rect { y: inner.y + inner.height - 1, height: 1, ..inner });
}
