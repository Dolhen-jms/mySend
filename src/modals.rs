//! Fenêtres modales : invite, confirmation, aide, machines, résultats, corbeille,
//! comparaisons, transferts, marque-pages.

use crate::app::{App, Severity};
use crate::fs::{errstr, FsRef};
use crate::input::Input;
use crate::style::*;
use crate::sync::{file_row, Itemized, Side};
use crate::trash::{self, TrashItem};
use crate::ui::{centered, dialog, markup, scroll_into_view, transfer_lines};
use crate::util::{basename, dirname, join, pad, truncate};
use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

pub struct Modal {
    pub id: u64,
    pub kind: ModalKind,
}

pub type SubmitFn = Box<dyn FnOnce(&mut App, Option<String>)>;
pub type ChangeFn = Box<dyn FnMut(&mut App, &str)>;
pub type CompleteFn = Box<dyn Fn(&App, &str) -> Option<(String, Vec<String>)>>;

pub struct Prompt {
    pub title: String,
    pub input: Input,
    pub candidates: Vec<String>,
    pub on_submit: Option<SubmitFn>,
    pub on_change: Option<ChangeFn>,
    pub complete: Option<CompleteFn>,
    /// Ligne de commande à la helix : affichée en bas de l'écran, après « : ».
    pub cmdline: bool,
}

impl Prompt {
    pub fn new(title: &str, value: &str, cursor: Option<usize>, on_submit: SubmitFn) -> Prompt {
        Prompt {
            title: title.to_string(),
            input: Input::new(value, cursor),
            candidates: Vec::new(),
            on_submit: Some(on_submit),
            on_change: None,
            complete: None,
            cmdline: false,
        }
    }
}

pub struct Confirm {
    pub msg: String,
    pub extra: Option<String>,
    pub on_yes: Option<Box<dyn FnOnce(&mut App)>>,
    pub on_no: Option<Box<dyn FnOnce(&mut App)>>,
}

#[derive(Default)]
pub struct Hosts {
    pub input: Input,
    pub focus_input: bool,
    /// machines cochées (Espace), par cible ; `None` = cette machine
    pub marked: std::collections::BTreeSet<Option<String>>,
    pub cursor: usize,
    pub offset: usize,
}

pub struct Results {
    pub title: String,
    pub items: Vec<String>,
    pub cwd: String,
    pub tab: u64,
    pub cursor: usize,
    pub offset: usize,
}

pub struct TrashM {
    pub fs: FsRef,
    pub items: Option<Vec<TrashItem>>,
    pub cursor: usize,
    pub offset: usize,
}

pub struct DirCmp {
    pub here: (FsRef, String),
    pub there: (FsRef, String),
    pub result: Option<Result<Itemized, String>>,
}

pub struct FileCmp {
    pub a: Side,
    pub b: Side,
    pub base_date: Option<f64>,
    pub content: Option<Vec<Line<'static>>>,
}

pub enum ModalKind {
    Prompt(Prompt),
    Confirm(Confirm),
    Help(crate::help::HelpM),
    Hosts(Hosts),
    Results(Results),
    Trash(TrashM),
    DirCmp(DirCmp, ),
    FileCmp(FileCmp),
    Transfers,
    Bookmarks(usize),
    /// Accès refusé : (FS, dossier) à rouvrir avec d'autres droits.
    Elevate(FsRef, String),
    /// Terminal intégré (shell dans le dossier) : désactivé, plus jamais créé (cf. `term.rs`).
    #[allow(dead_code)]
    Term(crate::term::Term),
    /// Envoi à un appareil Apple (omdrop).
    AirDrop(crate::airdrop_tui::AirModal),
}

// ---------------------------------------------------------------- clavier

/// Traite une touche ; renvoie faux si la fenêtre doit se fermer.
pub fn handle_key(app: &mut App, m: &mut Modal, k: &KeyEvent, name: &str) -> bool {
    let id = m.id;
    match &mut m.kind {
        ModalKind::Prompt(p) => match name {
            "escape" => {
                if let Some(f) = p.on_submit.take() {
                    f(app, None);
                }
                false
            }
            "enter" => {
                let text = p.input.text();
                if let Some(f) = p.on_submit.take() {
                    f(app, Some(text));
                }
                false
            }
            "tab" if p.complete.is_some() => {
                let text = p.input.text();
                if let Some((v, c)) = (p.complete.as_ref().unwrap())(app, &text) {
                    p.input.set(&v);
                    p.candidates = c;
                }
                true
            }
            _ => {
                if p.input.handle(k) {
                    let text = p.input.text();
                    if let Some(f) = &mut p.on_change {
                        f(app, &text);
                    }
                }
                true
            }
        },
        ModalKind::Confirm(c) => match name {
            "y" | "o" | "enter" => {
                if let Some(f) = c.on_yes.take() {
                    f(app);
                }
                false
            }
            "n" | "escape" | "q" => {
                if let Some(f) = c.on_no.take() {
                    f(app);
                }
                false
            }
            _ => true,
        },
        ModalKind::Help(h) => crate::help::key(h, k, name),
        ModalKind::Hosts(h) => {
            if h.focus_input {
                match name {
                    "escape" => false,
                    "enter" => {
                        let t = h.input.text().trim().to_string();
                        if t.is_empty() {
                            return true;
                        }
                        app.open_tab(Some(t.clone()), t, None, None);
                        false
                    }
                    "tab" | "shift+tab" => {
                        h.focus_input = false;
                        true
                    }
                    "up" => {
                        h.focus_input = false;
                        true
                    }
                    _ => {
                        h.input.handle(k);
                        true
                    }
                }
            } else {
                let n = app.hosts.len() + 1;
                match name {
                    "escape" => false,
                    "j" | "down" => {
                        h.cursor = (h.cursor + 1).min(n - 1);
                        true
                    }
                    "k" | "up" => {
                        h.cursor = h.cursor.saturating_sub(1);
                        true
                    }
                    "home" => {
                        h.cursor = 0;
                        true
                    }
                    "G" | "end" => {
                        h.cursor = n - 1;
                        true
                    }
                    "r" => {
                        app.start_discovery();
                        true
                    }
                    "a" | "tab" => {
                        h.focus_input = true;
                        true
                    }
                    "space" => {
                        let t = host_target(app, h.cursor);
                        if !h.marked.remove(&t) {
                            h.marked.insert(t);
                        }
                        h.cursor = (h.cursor + 1).min(n - 1);
                        true
                    }
                    "enter" => {
                        if h.marked.is_empty() {
                            open_host(app, h.cursor);
                        } else {
                            // dans l'ordre de la liste
                            let rows: Vec<usize> = (0..n).filter(|&i| h.marked.contains(&host_target(app, i))).collect();
                            for i in rows {
                                open_host(app, i);
                            }
                        }
                        false
                    }
                    // 1…9 puis 0 : ouvre directement la machine de cette ligne
                    d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => {
                        let i = (d.as_bytes()[0] - b'0') as usize;
                        let i = if i == 0 { 9 } else { i - 1 };
                        if i < n {
                            open_host(app, i);
                            return false;
                        }
                        true
                    }
                    _ => true,
                }
            }
        }
        ModalKind::Results(r) => match name {
            "escape" | "q" => false,
            "j" | "down" => {
                r.cursor = (r.cursor + 1).min(r.items.len().saturating_sub(1));
                true
            }
            "k" | "up" => {
                r.cursor = r.cursor.saturating_sub(1);
                true
            }
            "enter" => {
                if let Some(item) = r.items.get(r.cursor) {
                    if app.tabs.get(app.active).map(|t| t.id) == Some(r.tab) {
                        let p = join(&r.cwd, item.trim_end_matches('/'));
                        app.cd(dirname(&p), Some(basename(&p).to_string()));
                    }
                }
                false
            }
            _ => true,
        },
        ModalKind::Trash(t) => {
            let fs = t.fs.clone();
            let n = t.items.as_ref().map(|v| v.len()).unwrap_or(0);
            let cur = t.items.as_ref().and_then(|v| v.get(t.cursor).cloned());
            match name {
                "escape" | "q" | "T" => {
                    app.refresh_fs(&[fs]);
                    false
                }
                "j" | "down" => {
                    t.cursor = (t.cursor + 1).min(n.saturating_sub(1));
                    true
                }
                "k" | "up" => {
                    t.cursor = t.cursor.saturating_sub(1);
                    true
                }
                "enter" | "r" => {
                    if let Some(item) = cur {
                        match trash::restore(&fs, &item) {
                            Ok(d) => app.notify(&format!("Restauré : {d}"), Severity::Info, 4.0),
                            Err(e) => app.error(&errstr(&e)),
                        }
                        trash_reload(app, id, fs);
                    }
                    true
                }
                "D" => {
                    if let Some(item) = cur {
                        let msg = format!("Supprimer **définitivement** « {} » ?", item.name);
                        app.confirm(&msg, move |app| {
                            if let Err(e) = trash::delete(&fs, &item) {
                                app.error(&errstr(&e));
                            }
                            trash_reload(app, id, fs);
                        });
                    }
                    true
                }
                "V" => {
                    if n > 0 {
                        let msg = format!("Vider la corbeille de {} ({n} élément(s)) ?", fs.label);
                        app.confirm(&msg, move |app| {
                            if let Err(e) = trash::empty(&fs) {
                                app.error(&errstr(&e));
                            }
                            trash_reload(app, id, fs);
                        });
                    }
                    true
                }
                _ => true,
            }
        }
        ModalKind::DirCmp(d) => {
            if matches!(name, "escape" | "q") {
                return false;
            }
            let Some(Ok(res)) = &d.result else { return true };
            match name {
                ">" | "<" | "u" => {
                    app.dir_sync(d, name);
                    false
                }
                "m" => {
                    if res.only_there.is_empty() {
                        app.dir_sync(d, "m");
                    } else {
                        let msg = format!(
                            "Miroir : {} élément(s) seront **supprimés** sur {}. Continuer ?",
                            res.only_there.len(),
                            d.there.0.label
                        );
                        let dc = DirCmp { here: d.here.clone(), there: d.there.clone(), result: None };
                        app.confirm(&msg, move |app| app.dir_sync(&dc, "m"));
                    }
                    false
                }
                _ => true,
            }
        }
        ModalKind::FileCmp(fc) => {
            if matches!(name, "escape" | "q") {
                return false;
            }
            if fc.content.is_none() {
                return true;
            }
            match name {
                ">" => {
                    app.file_copy(&fc.a, &fc.b);
                    false
                }
                "<" => {
                    app.file_copy(&fc.b, &fc.a);
                    false
                }
                "u" => {
                    let (ma, mb) = (fc.a.entry.mtime, fc.b.entry.mtime);
                    if ma > mb {
                        app.file_copy(&fc.a, &fc.b);
                    } else if mb > ma {
                        app.file_copy(&fc.b, &fc.a);
                    } else {
                        app.notify("Même date des deux côtés : choisis > ou <.", Severity::Warning, 5.0);
                        return true;
                    }
                    false
                }
                "f" => {
                    app.file_merge(fc.a.clone(), fc.b.clone());
                    false
                }
                "e" => {
                    app.edit(vec![(fc.a.fs.clone(), fc.a.path.clone()), (fc.b.fs.clone(), fc.b.path.clone())], true);
                    false
                }
                _ => true,
            }
        }
        ModalKind::Transfers => match name {
            "escape" | "w" | "q" => false,
            "x" => {
                crate::transfers::cancel_all(&app.transfers);
                true
            }
            n if n.len() == 1 && ('1'..='9').contains(&n.chars().next().unwrap()) => {
                let i = n.parse::<usize>().unwrap() - 1;
                let tid = app.transfers.0.lock().unwrap().tasks.get(i).map(|t| t.id);
                if let Some(tid) = tid {
                    crate::transfers::cancel(&app.transfers, tid);
                }
                true
            }
            _ => true,
        },
        ModalKind::Term(t) => {
            t.key(k);
            true
        }
        ModalKind::AirDrop(a) => crate::airdrop_tui::key(app, a, name),
        ModalKind::Elevate(fs, path) => match name {
            "s" => {
                app.elevate_sudo(fs.clone(), path.clone());
                false
            }
            "u" => {
                app.elevate_user(fs.clone(), path.clone());
                false
            }
            "escape" | "n" | "q" => false,
            _ => true,
        },
        ModalKind::Bookmarks(cursor) => {
            let keys: Vec<String> = app.bookmarks.keys().cloned().collect();
            match name {
                "escape" => false,
                "down" => {
                    *cursor = (*cursor + 1).min(keys.len().saturating_sub(1));
                    true
                }
                "up" => {
                    *cursor = cursor.saturating_sub(1);
                    true
                }
                "enter" => {
                    if let Some(key) = keys.get(*cursor) {
                        app.goto_bookmark(key);
                        return false;
                    }
                    true
                }
                "delete" => {
                    if let Some(key) = keys.get(*cursor) {
                        app.bookmarks.remove(key);
                        crate::bookmarks::save(&app.bookmarks);
                        *cursor = (*cursor).min(keys.len().saturating_sub(2));
                    }
                    true
                }
                n if app.bookmarks.contains_key(n) => {
                    app.goto_bookmark(n);
                    false
                }
                _ => true,
            }
        }
    }
}

pub fn trash_reload(app: &mut App, id: u64, fs: FsRef) {
    app.send_thread(move || {
        let r = trash::list(&fs);
        Box::new(move |app: &mut App| {
            if let Some(ModalKind::Trash(t)) = app.modal_mut(id) {
                let items = r.unwrap_or_default();
                t.cursor = t.cursor.min(items.len().saturating_sub(1));
                t.items = Some(items);
            }
        })
    });
}

// ---------------------------------------------------------------- rendu

fn hint(text: &str) -> Line<'static> {
    Line::from(Span::styled(text.to_string(), fg(BRIGHT_BLACK)))
}

fn wrapped_height(lines: &[Line], w: usize) -> usize {
    lines.iter().map(|l| l.width().max(1).div_ceil(w.max(1))).sum()
}

/// Bordure « tall » de Textual pour les champs de saisie.
fn input_box(f: &mut Frame, area: Rect, input: &Input, placeholder: &str, focused: bool) {
    use ratatui::symbols::border;
    use ratatui::widgets::{Block, Borders};
    let set = border::Set {
        top_left: "▊",
        top_right: "▎",
        bottom_left: "▊",
        bottom_right: "▎",
        vertical_left: "▊",
        vertical_right: "▎",
        horizontal_top: "▔",
        horizontal_bottom: "▁",
    };
    let col = if focused { accent() } else { BRIGHT_BLACK };
    let block = Block::default().borders(Borders::ALL).border_set(set).border_style(fg(col));
    let inner = block.inner(area);
    f.render_widget(block, area);
    input.render(f, inner, Style::default(), placeholder, focused);
}

fn list_row(text: &str, w: usize, selected: bool) -> Line<'static> {
    let s = pad(&truncate(text, w), w);
    if selected {
        Line::from(Span::styled(s, Style::default().add_modifier(Modifier::REVERSED)))
    } else {
        Line::from(s)
    }
}

fn table_line(cells: &[(String, Style)], widths: &[usize], selected: bool) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (t, st)) in cells.iter().enumerate() {
        let w = widths[i];
        let mut st = *st;
        if selected {
            st = st.add_modifier(Modifier::REVERSED);
        }
        let sep = if i + 1 < cells.len() { " " } else { "" };
        spans.push(Span::styled(format!("{}{sep}", pad(&truncate(t, w), w)), st));
    }
    Line::from(spans)
}

fn col_widths(rows: &[Vec<(String, Style)>], total: usize, flex: usize) -> Vec<usize> {
    let n = rows.first().map(|r| r.len()).unwrap_or(0);
    let mut w: Vec<usize> = (0..n).map(|i| rows.iter().map(|r| r[i].0.width()).max().unwrap_or(0)).collect();
    let used: usize = w.iter().sum::<usize>() + n.saturating_sub(1);
    if used > total && n > 0 {
        let others: usize = w.iter().enumerate().filter(|(i, _)| *i != flex).map(|(_, x)| *x).sum::<usize>() + n - 1;
        w[flex] = total.saturating_sub(others).max(4);
    }
    w
}

pub fn render(f: &mut Frame, app: &mut App, idx: usize) {
    let full = f.area();
    let id = app.modals[idx].id;
    let _ = id;
    match &app.modals[idx].kind {
        ModalKind::Prompt(p) if p.cmdline => {
            let row = Rect { y: full.y + full.height.saturating_sub(1), height: 1.min(full.height), ..full };
            f.render_widget(ratatui::widgets::Clear, row);
            f.render_widget(Line::from(":"), row);
            p.input.render(f, Rect { x: row.x + 1, width: row.width.saturating_sub(1), ..row }, Style::default(), "", true);
        }
        ModalKind::Prompt(p) => {
            let w = 70.min(full.width as usize);
            let inner_w = w.saturating_sub(4);
            let cand_lines: Vec<Line> = if p.candidates.is_empty() {
                Vec::new()
            } else {
                let mut shown: Vec<String> = p.candidates.iter().take(60).cloned().collect();
                if p.candidates.len() > 60 {
                    shown.push("…".into());
                }
                vec![Line::from(Span::styled(shown.join("   "), dim()))]
            };
            let ch = wrapped_height(&cand_lines, inner_w);
            let title = Line::from(p.title.clone());
            let th = wrapped_height(std::slice::from_ref(&title), inner_w);
            let h = th + 3 + ch + 2;
            let inner = dialog(f, centered(full, w as u16, h as u16), None);
            f.render_widget(Paragraph::new(title).wrap(Wrap { trim: false }), Rect { height: th as u16, ..inner });
            let ib = Rect { y: inner.y + th as u16, height: 3, ..inner };
            input_box(f, ib, &p.input, "", true);
            if ch > 0 {
                f.render_widget(
                    Paragraph::new(cand_lines).wrap(Wrap { trim: false }),
                    Rect { y: ib.y + 3, height: ch as u16, ..inner },
                );
            }
        }
        ModalKind::Confirm(c) => {
            let w = 70.min(full.width as usize);
            let mut lines: Vec<Line> = c.msg.lines().map(markup).collect();
            if let Some(e) = &c.extra {
                lines.push(Line::from(Span::styled(e.clone(), fg(RED))));
            }
            let mh = wrapped_height(&lines, w - 4);
            lines.push(Line::from(""));
            lines.push(hint("y : oui    n : non"));
            let inner = dialog(f, centered(full, w as u16, (mh + 4) as u16), None);
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
        ModalKind::Help(_) => {
            if let ModalKind::Help(h) = &mut app.modals[idx].kind {
                crate::help::render(f, h);
            }
        }
        ModalKind::Hosts(_) => render_hosts(f, app, idx),
        ModalKind::Results(_) => {
            let area = centered(full, full.width * 9 / 10, full.height * 8 / 10);
            let inner = dialog(f, area, None);
            let ModalKind::Results(r) = &mut app.modals[idx].kind else { return };
            f.render_widget(Line::from(Span::styled(r.title.clone(), b())), Rect { height: 1, ..inner });
            let lh = inner.height.saturating_sub(3) as usize;
            scroll_into_view(r.cursor, &mut r.offset, lh, r.items.len(), 0);
            let lines: Vec<Line> = r
                .items
                .iter()
                .enumerate()
                .skip(r.offset)
                .take(lh)
                .map(|(i, s)| list_row(s, inner.width as usize, i == r.cursor))
                .collect();
            f.render_widget(Paragraph::new(lines), Rect { y: inner.y + 1, height: lh as u16, ..inner });
            f.render_widget(hint("Entrée aller au fichier   Échap fermer"), Rect { y: inner.y + inner.height - 1, height: 1, ..inner });
        }
        ModalKind::Trash(_) => {
            let area = centered(full, full.width * 9 / 10, full.height * 8 / 10);
            let inner = dialog(f, area, None);
            let ModalKind::Trash(t) = &mut app.modals[idx].kind else { return };
            let status = match &t.items {
                None => Line::from(format!("Corbeille de {}… lecture", t.fs.label)),
                Some(v) => markup(&format!("Corbeille de **{}** : {} élément(s)", t.fs.label, v.len())),
            };
            f.render_widget(status, Rect { height: 1, ..inner });
            let lh = inner.height.saturating_sub(5) as usize;
            let items = t.items.clone().unwrap_or_default();
            let mut rows = vec![vec![
                ("Nom".to_string(), b()),
                ("Emplacement d'origine".to_string(), b()),
                ("Supprimé le".to_string(), b()),
            ]];
            for it in &items {
                rows.push(vec![
                    (it.name.clone(), Style::default()),
                    (t.fs.pretty(&it.path), Style::default()),
                    (it.date.clone(), Style::default()),
                ]);
            }
            let widths = col_widths(&rows, inner.width as usize, 1);
            scroll_into_view(t.cursor, &mut t.offset, lh, items.len(), 0);
            let mut lines = vec![table_line(&rows[0], &widths, false)];
            for (i, r) in rows.iter().enumerate().skip(1 + t.offset).take(lh) {
                lines.push(table_line(r, &widths, i - 1 == t.cursor));
            }
            f.render_widget(Paragraph::new(lines), Rect { y: inner.y + 2, height: (lh + 1) as u16, ..inner });
            f.render_widget(
                hint("Entrée/r restaurer   D supprimer définitivement   V vider la corbeille   Échap fermer"),
                Rect { y: inner.y + inner.height - 1, height: 1, ..inner },
            );
        }
        ModalKind::DirCmp(d) => {
            let area = centered(full, full.width * 9 / 10, full.height * 8 / 10);
            let inner = dialog(f, area, None);
            let mut head = vec![
                Line::from(vec![Span::styled("ici     ", bold(GREEN)), Span::raw(format!("{}:{}", d.here.0.label, d.here.1))]),
                Line::from(vec![Span::styled("là-bas  ", bold(RED)), Span::raw(format!("{}:{}", d.there.0.label, d.there.1))]),
            ];
            head.push(Line::from(""));
            let mut body: Vec<Line> = Vec::new();
            match &d.result {
                None => body.push(Line::from(Span::styled("comparaison en cours…", dim_italic()))),
                Some(Err(e)) => body.push(Line::from(Span::styled(e.clone(), fg(RED)))),
                Some(Ok(r)) => {
                    if r.only_here.is_empty() && r.differ.is_empty() && r.only_there.is_empty() {
                        body.push(Line::from(Span::styled("Identiques.", fg(GREEN))));
                    }
                    for (title, list, col) in [
                        ("Seulement ici", &r.only_here, GREEN),
                        ("Différents (taille ou date)", &r.differ, YELLOW),
                        ("Seulement là-bas", &r.only_there, RED),
                    ] {
                        if list.is_empty() {
                            continue;
                        }
                        body.push(Line::from(Span::styled(format!("{title} ({})", list.len()), bold(col))));
                        for n in list.iter().take(500) {
                            body.push(Line::from(Span::styled(format!("  {n}"), fg(col))));
                        }
                        if list.len() > 500 {
                            body.push(Line::from(Span::styled(format!("  … et {} autres", list.len() - 500), dim())));
                        }
                    }
                }
            }
            let hints = [
                "> envoyer ici → là-bas   < récupérer là-bas → ici   u fusionner (le plus récent gagne)",
                "m miroir ici → là-bas (supprime ce qui est en trop là-bas)   Échap fermer",
            ];
            let hh = 2u16;
            f.render_widget(Paragraph::new(head), Rect { height: 3, ..inner });
            let bh = inner.height.saturating_sub(3 + hh + 1);
            f.render_widget(Paragraph::new(body), Rect { y: inner.y + 3, height: bh, ..inner });
            f.render_widget(
                Paragraph::new(hints.iter().map(|h| hint(h)).collect::<Vec<_>>()),
                Rect { y: inner.y + inner.height - hh, height: hh, ..inner },
            );
        }
        ModalKind::FileCmp(fc) => {
            let area = centered(full, full.width * 9 / 10, full.height * 8 / 10);
            let inner = dialog(f, area, None);
            let (ma, mb) = (fc.a.entry.mtime, fc.b.entry.mtime);
            let mut head = vec![file_row("ici", &fc.a, ma > mb), file_row("là-bas", &fc.b, mb > ma)];
            let anc = match fc.base_date {
                Some(t) => format!(
                    "connu (dernière synchro par mySend le {}) : f fusionne tout seul ce qui n'a changé que d'un côté",
                    crate::util::fmt_time(t, "%Y-%m-%d %H:%M")
                ),
                None => "inconnu (jamais synchronisés par mySend) : avec f, chaque différence sera un conflit à trancher".into(),
            };
            head.push(Line::from(vec![Span::styled(format!("{:<8}", "ancêtre"), b()), Span::styled(anc, dim())]));
            head.push(Line::from(""));
            let hh = inner.height.min(5);
            f.render_widget(Paragraph::new(head).wrap(Wrap { trim: false }), Rect { height: hh, ..inner });
            let body = fc.content.clone().unwrap_or_else(|| vec![Line::from(Span::styled("chargement…", dim_italic()))]);
            let bh = inner.height.saturating_sub(hh + 2);
            f.render_widget(Paragraph::new(body), Rect { y: inner.y + hh, height: bh, ..inner });
            f.render_widget(
                hint("> copier ici → là-bas   < copier là-bas → ici   u le plus récent gagne   f fusionner   e côte à côte   Échap fermer"),
                Rect { y: inner.y + inner.height - 1, height: 1, ..inner },
            );
        }
        ModalKind::Transfers => {
            let area = centered(full, full.width * 9 / 10, full.height * 8 / 10);
            let inner = dialog(f, area, None);
            let st = app.transfers.0.lock().unwrap();
            let mut lines = if st.tasks.is_empty() {
                vec![Line::from(Span::styled("Aucun transfert en cours.", dim_italic()))]
            } else {
                transfer_lines(&st.tasks, inner.width as usize, true)
            };
            lines.push(Line::from(""));
            let log: Vec<Line> = st
                .log
                .iter()
                .map(|(t, k)| {
                    let s = match k {
                        crate::transfers::LogKind::Plain => Style::default(),
                        crate::transfers::LogKind::Bold => b(),
                        crate::transfers::LogKind::Green => fg(GREEN),
                        crate::transfers::LogKind::Red => fg(RED),
                    };
                    Line::from(Span::styled(t.clone(), s))
                })
                .collect();
            let avail = inner.height.saturating_sub(2) as usize;
            let room = avail.saturating_sub(lines.len());
            let skip = log.len().saturating_sub(room);
            lines.extend(log.into_iter().skip(skip));
            f.render_widget(Paragraph::new(lines), Rect { height: avail as u16, ..inner });
            f.render_widget(
                hint("1…9 annuler la tâche   x tout annuler   Échap fermer"),
                Rect { y: inner.y + inner.height - 1, height: 1, ..inner },
            );
        }
        ModalKind::Term(_) => {
            if let ModalKind::Term(t) = &mut app.modals[idx].kind {
                t.render(f);
            }
        }
        ModalKind::AirDrop(a) => crate::airdrop_tui::render(f, a),
        ModalKind::Elevate(fs, path) => {
            let w = 70.min(full.width as usize);
            let mut lines = vec![
                markup(&format!("Accès refusé : **{}:{}**", fs.label, fs.pretty(path))),
                Line::from("Rouvrir ce dossier dans un nouvel onglet avec d'autres droits ?"),
            ];
            let mh = wrapped_height(&lines, w - 4);
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled("s", bold(YELLOW)), Span::raw("  sudo (mot de passe de ton compte)")]));
            lines.push(Line::from(vec![Span::styled("u", bold(YELLOW)), Span::raw("  autre utilisateur ssh (son mot de passe)")]));
            lines.push(Line::from(""));
            lines.push(hint("s sudo    u autre utilisateur    Échap annuler"));
            let inner = dialog(f, centered(full, w as u16, (mh + 7) as u16), None);
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
        ModalKind::Bookmarks(cursor) => {
            let w = 90.min(full.width) as usize;
            let cursor = *cursor;
            let mut rows = vec![vec![("Touche".to_string(), b()), ("Machine".to_string(), b()), ("Dossier".to_string(), b())]];
            for (k, bm) in &app.bookmarks {
                let fs = app.fs_for(bm.target.as_deref());
                let dir = fs.map(|f| f.pretty(&bm.path)).unwrap_or_else(|| bm.path.clone());
                rows.push(vec![(k.clone(), bold(YELLOW)), (bm.label.clone(), Style::default()), (dir, Style::default())]);
            }
            let n = rows.len() - 1;
            let shown = n.clamp(1, 20);
            let h = (shown + 1 + 4 + 2).min(full.height as usize * 8 / 10);
            let inner = dialog(f, centered(full, w as u16, h as u16), Some("Marque-pages"));
            let widths = col_widths(&rows, inner.width as usize, 2);
            let mut lines = vec![table_line(&rows[0], &widths, false)];
            if n == 0 {
                lines.push(Line::from(Span::styled("aucun marque-page", dim_italic())));
            } else {
                let off = cursor.saturating_sub(shown - 1);
                for (i, r) in rows.iter().enumerate().skip(1 + off).take(shown) {
                    lines.push(table_line(r, &widths, i - 1 == cursor));
                }
            }
            let th = inner.height.saturating_sub(3);
            f.render_widget(Paragraph::new(lines), Rect { height: th, ..inner });
            f.render_widget(
                Paragraph::new(vec![
                    hint("lettre ou Entrée y aller   Suppr effacer   Échap fermer"),
                    Line::from(Span::styled("Nouveau : m puis une lettre, dans le dossier à retenir.", dim())),
                ]),
                Rect { y: inner.y + inner.height - 2, height: 2, ..inner },
            );
        }
    }
}

/// Ouvre un onglet sur la ligne `i` du sélecteur (0 = cette machine).
fn open_host(app: &mut App, i: usize) {
    if i == 0 {
        app.open_tab(None, "local".into(), None, None);
    } else if let Some(host) = app.hosts.get(i - 1).cloned() {
        app.open_tab(Some(host.target.clone()), host.label(), None, None);
    }
}

/// Cible de la ligne `i` du sélecteur (`None` = cette machine).
fn host_target(app: &App, i: usize) -> Option<String> {
    if i == 0 { None } else { app.hosts.get(i - 1).map(|h| h.target.clone()) }
}

/// Raccourci de la ligne `i` : 1…9, puis 0 pour la dixième.
fn host_key(i: usize) -> String {
    match i {
        0..=8 => (i + 1).to_string(),
        9 => "0".into(),
        _ => String::new(),
    }
}

fn render_hosts(f: &mut Frame, app: &mut App, idx: usize) {
    let full = f.area();
    let area = centered(full, full.width * 9 / 10, full.height * 8 / 10);
    let inner = dialog(f, area, None);
    let status = if app.discovering {
        "Recherche des machines (mDNS, ~/.ssh/config)…".to_string()
    } else {
        let r = app.hosts.iter().filter(|h| h.reachable).count();
        format!("{} machine(s), dont {r} joignable(s).", app.hosts.len())
    };
    f.render_widget(Line::from(status), Rect { height: 1, ..inner });
    let marked = match &app.modals[idx].kind {
        ModalKind::Hosts(h) => h.marked.clone(),
        _ => return,
    };
    let mark = |t: Option<String>| if marked.contains(&t) { ("✓".to_string(), fg(GREEN).add_modifier(Modifier::BOLD)) } else { (" ".to_string(), Style::default()) };
    let mut rows = vec![vec![
        (String::new(), b()),
        (String::new(), b()),
        (String::new(), b()),
        ("Machine".to_string(), b()),
        ("Adresse".to_string(), b()),
        ("Port".to_string(), b()),
        ("Vue par".to_string(), b()),
    ]];
    rows.push(vec![
        (host_key(0), fg(YELLOW)),
        mark(None),
        ("●".into(), fg(GREEN)),
        ("local".into(), Style::default()),
        (String::new(), Style::default()),
        (String::new(), Style::default()),
        ("cette machine".into(), Style::default()),
    ]);
    for (i, h) in app.hosts.iter().enumerate() {
        rows.push(vec![
            (host_key(i + 1), fg(YELLOW)),
            mark(Some(h.target.clone())),
            if h.reachable { ("●".into(), fg(GREEN)) } else { ("○".into(), fg(RED)) },
            (h.label(), Style::default()),
            (h.address.clone().unwrap_or_else(|| "nom introuvable".into()), Style::default()),
            (h.port.to_string(), Style::default()),
            (h.sources.iter().cloned().collect::<Vec<_>>().join(", "), Style::default()),
        ]);
    }
    let widths = col_widths(&rows, inner.width as usize, 5);
    let ModalKind::Hosts(h) = &mut app.modals[idx].kind else { return };
    let n = rows.len() - 1;
    h.cursor = h.cursor.min(n - 1);
    let th = inner.height.saturating_sub(1 + 3 + 2 + 1) as usize;
    scroll_into_view(h.cursor, &mut h.offset, th, n, 0);
    let mut lines = vec![table_line(&rows[0], &widths, false)];
    for (i, r) in rows.iter().enumerate().skip(1 + h.offset).take(th) {
        lines.push(table_line(r, &widths, i - 1 == h.cursor && !h.focus_input));
    }
    f.render_widget(Paragraph::new(lines), Rect { y: inner.y + 1, height: (th + 1) as u16, ..inner });
    let ib = Rect { y: inner.y + inner.height - 5, height: 3, ..inner };
    input_box(f, ib, &h.input, "… ou une adresse : alias ssh, IP, user@hôte  (Entrée)", h.focus_input);
    f.render_widget(
        hint("1-9 0 ou Entrée ouvrir   Espace cocher (Entrée ouvre les cochées)   r relancer la recherche   a saisir une adresse   Échap fermer"),
        Rect { y: inner.y + inner.height - 1, height: 1, ..inner },
    );
}



