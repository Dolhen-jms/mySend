//! Rendu de l'écran principal.

use crate::app::{arrange, App, Severity, SortKey, Tab};
use crate::config::SCROLLOFF;
use crate::fs::{same, Entry};
use crate::preview::Preview;
use crate::style::*;
use crate::transfers::TaskInfo;
use crate::util::{fmt_time, human, join, pad, truncate};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;
use ratatui_image::StatefulImage;
use unicode_width::UnicodeWidthStr;

pub fn centered(full: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(full.width);
    let h = h.min(full.height);
    Rect { x: full.x + (full.width - w) / 2, y: full.y + (full.height - h) / 2, width: w, height: h }
}

/// Cadre de dialogue : fond noir, bordure arrondie jaune, marge horizontale 1.
pub fn dialog(f: &mut Frame, area: Rect, title: Option<&str>) -> Rect {
    f.render_widget(Clear, area);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(fg(accent()))
        .style(Style::default().bg(surface()))
        .padding(Padding::horizontal(1));
    if let Some(t) = title {
        block = block.title(Span::styled(format!(" {t} "), b()));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

/// `**gras**` → spans.
pub fn markup(s: &str) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, part) in s.split("**").enumerate() {
        if part.is_empty() {
            continue;
        }
        spans.push(if i % 2 == 1 { Span::styled(part.to_string(), b()) } else { Span::raw(part.to_string()) });
    }
    Line::from(spans)
}

pub fn scroll_into_view(cursor: usize, offset: &mut usize, h: usize, n: usize, so: usize) {
    if h == 0 {
        return;
    }
    let so = so.min((h - 1) / 2);
    if cursor < *offset + so {
        *offset = cursor.saturating_sub(so);
    } else if cursor + so >= *offset + h {
        *offset = cursor + so + 1 - h;
    }
    *offset = (*offset).min(n.saturating_sub(h));
}

fn pill(text: &str, color: Color) -> Vec<Span<'static>> {
    if use_icons() {
        vec![
            Span::styled("\u{e0b6}", fg(color)),
            Span::styled(text.to_string(), Style::default().fg(BLACK).bg(color).add_modifier(Modifier::BOLD)),
            Span::styled("\u{e0b4}", fg(color)),
        ]
    } else {
        vec![Span::styled(format!(" {text} "), bold(color).add_modifier(Modifier::REVERSED))]
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    None,
    Sel,
    Copy,
    Cut,
}

/// Une ligne d'entrée (marque, icône, nom, taille).
fn entry_line(e: &Entry, w: usize, mark: Mark, show_size: bool, cursor: Option<Style>) -> Line<'static> {
    let (mch, mst) = match mark {
        Mark::None => (" ", Style::default()),
        Mark::Sel => ("▍", bold(accent())),
        Mark::Copy => ("▍", bold(GREEN)),
        Mark::Cut => ("▍", bold(RED)),
    };
    let size = if show_size && !e.is_dir { human(e.size) } else { String::new() };
    let lead = if use_icons() { 4 } else { 2 };
    let nw = w.saturating_sub(lead + 1 + size.width()).max(1);
    let mut nst = name_style(e);
    if e.name.starts_with('.') {
        nst = nst.add_modifier(Modifier::DIM);
    }
    let mut spans = vec![Span::styled(mch, mst), Span::raw(" ")];
    if use_icons() {
        let (g, c) = icon(e);
        spans.push(Span::styled(format!("{g} "), fg(c)));
    }
    spans.push(Span::styled(pad(&truncate(&e.name, nw), nw), nst));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(size, dim()));
    if let Some(cs) = cursor {
        for s in spans.iter_mut().skip(1) {
            s.style = s.style.patch(cs);
        }
        // complète la ligne jusqu'au bord
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        if used < w {
            spans.push(Span::styled(" ".repeat(w - used), cs));
        }
    }
    Line::from(spans)
}

fn mark_of(app: &App, tab: &Tab, path: &str, idx: Option<usize>) -> Mark {
    if tab.selected.contains_key(path) {
        return Mark::Sel;
    }
    if let (Some(i), Some((a, b))) = (idx, tab.visual_range()) {
        if i >= a && i <= b {
            return Mark::Sel;
        }
    }
    if let Some(y) = &app.yank {
        if same(&y.fs, &tab.fs) && y.items.contains_key(path) {
            return if y.cut { Mark::Cut } else { Mark::Copy };
        }
    }
    Mark::None
}

fn empty_line(msg: &str) -> Line<'static> {
    Line::from(Span::styled(format!(" {msg}"), dim_italic()))
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let full = f.area();
    let tasks_n = app.transfers.0.lock().unwrap().tasks.len();
    let trans_h = if tasks_n > 0 { tasks_n.min(8) as u16 + 1 } else { 0 };
    let hints = app.pending.map(|p| hint_pairs(app, p));
    let hints_h = hints.as_ref().map(|h| h.len().div_ceil(3).max(1) as u16 + 2).unwrap_or(0);
    let [header, body, trans, hint_area, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(trans_h),
        Constraint::Length(hints_h),
        Constraint::Length(1),
    ])
    .areas(full);

    if app.tabs.is_empty() {
        return;
    }
    draw_header(f, app, header);
    let [pa, ca, va] = Layout::horizontal([Constraint::Fill(2), Constraint::Fill(4), Constraint::Fill(4)]).areas(body);
    // colonne courante encadrée par des traits verticaux
    let cur_block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT)
        .border_set(ratatui::symbols::border::Set {
            vertical_left: "▏",
            vertical_right: "▕",
            ..ratatui::symbols::border::PLAIN
        })
        .border_style(fg(BRIGHT_BLACK));
    let ci = cur_block.inner(ca);
    f.render_widget(cur_block, ca);
    app.areas.parent = pa;
    app.areas.current = ci;
    app.areas.preview = va;
    app.col_height = ci.height as usize;

    draw_current(f, app, ci);
    draw_parent(f, app, pa);
    draw_preview(f, app, va);
    if trans_h > 0 {
        draw_transfers(f, app, trans);
    }
    if let Some(h) = hints {
        draw_hints(f, app, hint_area, &h);
    }
    draw_status(f, app, status);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();
    for (i, t) in app.tabs.iter().enumerate() {
        let (glyph, col) = if t.fs.remote { ('\u{f233}', MAGENTA) } else { ('\u{f108}', GREEN) };
        let text = if use_icons() { format!("{} {glyph} {}", i + 1, t.fs.label) } else { format!("{} {}", i + 1, t.fs.label) };
        if i == app.active {
            spans.extend(pill(&text, col));
        } else {
            spans.push(Span::styled(format!(" {text} "), dim()));
        }
    }
    spans.push(Span::raw(" "));
    let t = app.tab();
    let p = t.fs.pretty(&t.cwd);
    if p == "/" {
        spans.push(Span::styled("/", b()));
    } else if let Some(i) = p.rfind('/') {
        spans.push(Span::styled(p[..=i].to_string(), dim()));
        spans.push(Span::styled(p[i + 1..].to_string(), b()));
    } else {
        spans.push(Span::styled(p, b()));
    }
    f.render_widget(Line::from(truncate_spans(spans, area.width as usize)), area);
}

fn truncate_spans(spans: Vec<Span<'static>>, w: usize) -> Vec<Span<'static>> {
    let total: usize = spans.iter().map(|s| s.content.width()).sum();
    if total <= w {
        return spans;
    }
    let mut out = Vec::new();
    let mut used = 0;
    for s in spans {
        let sw = s.content.width();
        if used + sw < w {
            used += sw;
            out.push(s);
        } else {
            let rest = w.saturating_sub(used);
            out.push(Span::styled(truncate(&s.content, rest), s.style));
            break;
        }
    }
    out
}

fn draw_current(f: &mut Frame, app: &mut App, area: Rect) {
    let h = area.height as usize;
    let w = area.width as usize;
    let (cursor, n) = (app.tab().cursor, app.tab().entries.len());
    {
        let t = app.tab_mut();
        scroll_into_view(cursor, &mut t.offset, h, n, SCROLLOFF);
    }
    let t = app.tab();
    if n == 0 {
        let msg = if t.raw.is_none() {
            "chargement…"
        } else if !t.filter.is_empty() {
            "aucun résultat"
        } else {
            "dossier vide"
        };
        f.render_widget(empty_line(msg), area);
        return;
    }
    let rev = cursor_style();
    let lines: Vec<Line> = t
        .entries
        .iter()
        .enumerate()
        .skip(t.offset)
        .take(h)
        .map(|(i, e)| {
            let mark = mark_of(app, t, &t.path_of(&e.name), Some(i));
            entry_line(e, w, mark, true, if i == cursor { Some(rev) } else { None })
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_parent(f: &mut Frame, app: &mut App, area: Rect) {
    let h = area.height as usize;
    let w = area.width as usize;
    let Some(pdir) = app.tab().parent_dir() else { return };
    let name = crate::util::basename(&app.tab().cwd).to_string();
    let pc = app.tab().parent.iter().position(|e| e.name == name);
    let n = app.tab().parent.len();
    if let Some(c) = pc {
        let t = app.tab_mut();
        scroll_into_view(c, &mut t.parent_offset, h, n, SCROLLOFF);
    }
    let t = app.tab();
    let cs = Style::default().bg(parent_cursor_bg());
    let lines: Vec<Line> = t
        .parent
        .iter()
        .enumerate()
        .skip(t.parent_offset)
        .take(h)
        .map(|(i, e)| {
            let mark = mark_of(app, t, &join(&pdir, &e.name), None);
            entry_line(e, w, mark, false, if Some(i) == pc { Some(cs) } else { None })
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_preview(f: &mut Frame, app: &mut App, area: Rect) {
    let Some((key, p)) = app.preview.current.clone() else { return };
    if app.hover_key().as_ref() != Some(&key) {
        return;
    }
    let w = area.width as usize;
    let h = area.height as usize;
    match &*p {
        Preview::Error(e) => f.render_widget(Paragraph::new(crate::preview::error_line(e.as_str())).wrap(Wrap { trim: false }), area),
        Preview::Special => f.render_widget(crate::preview::special_line("fichier spécial"), area),
        Preview::Binary { entry } => {
            let mut perms_line = vec![];
            perms_line.extend(perms(entry.mode));
            let lines = vec![
                Line::from(Span::styled("fichier binaire", dim_italic())),
                Line::from(""),
                Line::from(human(entry.size)),
                Line::from(perms_line),
                Line::from(fmt_time(entry.mtime, "%Y-%m-%d %H:%M")),
                Line::from(""),
                Line::from(Span::styled("o : ouvrir avec l'application par défaut", dim())),
            ];
            f.render_widget(Paragraph::new(lines), area);
        }
        Preview::Text { lines } => {
            let ls: Vec<Line> = lines.iter().take(h).cloned().collect();
            f.render_widget(Paragraph::new(ls), area);
        }
        Preview::Dir { entries } => {
            let t = app.tab();
            let list = arrange(entries, t.hidden, "", t.sort, None);
            let dk = (key.0, key.1.clone(), key.2);
            let mut head = vec![Span::styled(format!("{} élément(s) · ", list.len()), dim())];
            match app.preview.du.get(&dk) {
                Some(s) => head.push(Span::styled(human(*s), b())),
                None => head.push(Span::styled("calcul de la taille…", dim_italic())),
            }
            let mut lines = vec![Line::from(head), Line::from("")];
            if list.is_empty() {
                lines.push(empty_line("dossier vide"));
            } else {
                for e in list.iter().take(h.saturating_sub(2)) {
                    lines.push(entry_line(e, w, Mark::None, true, None));
                }
            }
            f.render_widget(Paragraph::new(lines), area);
        }
        Preview::Image { dims, size, image, key } => {
            let mut head = String::new();
            if let Some((x, y)) = dims {
                head.push_str(&format!("{x}×{y}  ·  "));
            }
            head.push_str(&human(*size));
            f.render_widget(Line::from(Span::styled(head, dim())), Rect { height: 1, ..area });
            let Some(img) = image else { return };
            if app.preview.image.as_ref().map(|(k, _)| k != key).unwrap_or(true) {
                let proto = app.picker.new_resize_protocol(img.clone());
                app.preview.image = Some((key.clone(), proto));
            }
            if let Some((_, proto)) = &mut app.preview.image {
                let ia = Rect { y: area.y + 1, height: area.height.saturating_sub(1), ..area };
                f.render_stateful_widget(StatefulImage::default(), ia, proto);
            }
        }
    }
}

/// Lignes du tableau des transferts.
pub fn transfer_lines(tasks: &[TaskInfo], width: usize, numbered: bool) -> Vec<Line<'static>> {
    let num_w = if numbered { 4 } else { 0 };
    let avail = width.saturating_sub(num_w + 5 + 3);
    let name_w = avail * 2 / 7;
    let bar_w = avail * 3 / 7;
    let extra_w = avail.saturating_sub(name_w + bar_w);
    let mut out = Vec::new();
    for (i, t) in tasks.iter().enumerate() {
        let mut spans = Vec::new();
        if numbered {
            spans.push(Span::styled(format!("{:<4}", format!("{}.", i + 1)), bold(YELLOW)));
        }
        match &t.running {
            None => {
                spans.push(Span::styled(pad(&truncate(&t.title, name_w), name_w), dim()));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(pad("en attente…", bar_w), dim_italic()));
            }
            Some(r) => {
                spans.push(Span::styled(pad(&truncate(&format!("⇅ {}", r.label), name_w), name_w), b()));
                spans.push(Span::raw(" "));
                match r.pct {
                    Some(p) => {
                        let fill = bar_w * p as usize / 100;
                        spans.push(Span::styled("━".repeat(fill), fg(GREEN)));
                        spans.push(Span::styled("━".repeat(bar_w - fill), fg(BRIGHT_BLACK)));
                        spans.push(Span::raw(format!(" {:>5}", format!("{p}%"))));
                    }
                    None => {
                        let seg = (bar_w / 4).max(1);
                        let ms = r.started.elapsed().as_millis() as usize;
                        let pos = (ms / 50) % (bar_w + seg).max(1);
                        let mut s = Vec::new();
                        for x in 0..bar_w {
                            let on = x + seg >= pos && x < pos;
                            s.push(Span::styled("━", fg(if on { MAGENTA } else { BRIGHT_BLACK })));
                        }
                        spans.extend(s);
                        spans.push(Span::raw("      "));
                    }
                }
                spans.push(Span::raw(" "));
                spans.push(Span::styled(truncate(&r.extra, extra_w), dim()));
            }
        }
        out.push(Line::from(spans));
    }
    out
}

fn draw_transfers(f: &mut Frame, app: &App, area: Rect) {
    let top = Rect { height: 1, ..area };
    f.render_widget(Line::from(Span::styled("▔".repeat(area.width as usize), fg(BRIGHT_BLACK))), top);
    let st = app.transfers.0.lock().unwrap();
    let lines = transfer_lines(&st.tasks[..st.tasks.len().min(8)], area.width as usize, false);
    f.render_widget(Paragraph::new(lines), Rect { y: area.y + 1, height: area.height - 1, ..area });
}

pub fn prefix_desc(p: char) -> &'static str {
    match p {
        'g' => "aller à",
        'c' => "copier",
        ',' => "trier",
        'm' => "marque-page sur la touche",
        '=' => "comparer le dossier avec l'onglet",
        '-' => "comparer le fichier avec l'onglet",
        _ => "",
    }
}

fn hint_pairs(app: &App, p: char) -> Vec<(String, String)> {
    let s = |a: &str, b: &str| (a.to_string(), b.to_string());
    match p {
        'g' => vec![
            s("g", "tout en haut"),
            s("Espace", "aller à… (chemin saisi, Tab complète)"),
            s("h", "~ (dossier personnel)"),
            s("c", "~/.config"),
            s("d", "~/Downloads"),
            s("/", "racine /"),
        ],
        'c' => {
            let remote = app.tab().fs.remote;
            vec![
                s("c", if remote { "chemin complet (hôte:chemin)" } else { "chemin complet" }),
                s("f", "nom du fichier"),
                s("d", "chemin du dossier courant"),
            ]
        }
        ',' => vec![s("n", "nom"), s("m", "date de modification"), s("s", "taille"), s("N M S", "idem, ordre inversé")],
        'm' => {
            let mut v: Vec<(String, String)> =
                app.bookmarks.iter().map(|(k, b)| (k.clone(), format!("remplacer ({}:{})", b.label, b.path))).collect();
            v.push(s("a-z 0-9", "nouvelle lettre pour ce dossier"));
            v
        }
        '=' | '-' => {
            let mut v: Vec<(String, String)> = app
                .tabs
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != app.active && *i < 9)
                .map(|(i, t)| ((i + 1).to_string(), format!("{}:{}", t.fs.label, t.fs.pretty(&t.cwd))))
                .collect();
            if v.is_empty() {
                v.push(s("", "ouvre d'abord un autre onglet (t)"));
            } else if p == '=' {
                v.push(s("", "dossier : > < u m dans la fenêtre qui s'ouvre"));
            } else {
                v.push(s("", "fichier sous le curseur : > < u f e dans la fenêtre qui s'ouvre"));
            }
            v
        }
        _ => Vec::new(),
    }
}

fn draw_hints(f: &mut Frame, app: &App, area: Rect, pairs: &[(String, String)]) {
    let p = app.pending.unwrap_or(' ');
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(fg(accent()))
        .title(Span::styled(format!(" {p} … {}   (Échap : annuler) ", prefix_desc(p)), fg(accent())));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = pairs.len().div_ceil(3).max(1);
    let cols = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1), Constraint::Fill(1)]).split(inner);
    for (c, chunk) in pairs.chunks(rows).enumerate() {
        let kw = chunk.iter().map(|(k, _)| k.width()).max().unwrap_or(0);
        let lines: Vec<Line> = chunk
            .iter()
            .map(|(k, d)| {
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(pad(k, kw), bold(YELLOW)),
                    Span::raw("  "),
                    Span::raw(d.clone()),
                ])
            })
            .collect();
        if c < cols.len() {
            f.render_widget(Paragraph::new(lines), cols[c]);
        }
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let t = app.tab();
    let mode_col = if t.visual.is_some() {
        MAGENTA
    } else if !t.selected.is_empty() {
        YELLOW
    } else {
        BLUE
    };
    let mode = if t.visual.is_some() {
        "VIS"
    } else if !t.selected.is_empty() {
        "SEL"
    } else {
        "NOR"
    };
    let mut left = pill(mode, mode_col);
    if let Some(e) = t.current() {
        left.push(Span::raw("  "));
        if !e.is_dir {
            left.push(Span::raw(format!("{}  ", human(e.size))));
        }
        left.extend(perms(e.mode));
    }

    let mut right: Vec<Span> = Vec::new();
    if !t.filter.is_empty() {
        let txt = if use_icons() { format!("\u{f0b0} {}  ", t.filter) } else { format!("filtre : {}  ", t.filter) };
        right.push(Span::styled(txt, fg(YELLOW)));
    }
    let nsel = t.selected.len();
    if nsel > 0 {
        right.push(Span::styled(format!("{nsel} coché(s)  "), bold(YELLOW)));
    }
    if let Some(y) = &app.yank {
        let (w, c) = if y.cut { ("coupé(s)", RED) } else { ("copié(s)", GREEN) };
        right.push(Span::styled(format!("{} {w} [{}]  ", y.items.len(), y.fs.label), bold(c)));
    }
    if t.sort != (SortKey::Natural, false) {
        let mut s = format!("↕{}", t.sort.0.name());
        if t.sort.1 {
            s.push('↑');
        }
        s.push_str("  ");
        right.push(Span::styled(s, dim()));
    }
    if let Some(e) = t.current() {
        right.push(Span::styled(format!("{}  ", fmt_time(e.mtime, "%Y-%m-%d %H:%M")), dim()));
    }
    let n = t.entries.len();
    let h = app.col_height;
    let pos = if n <= h {
        "Tout".to_string()
    } else if t.cursor == 0 {
        "Haut".to_string()
    } else if t.cursor + 1 == n {
        "Bas".to_string()
    } else {
        format!("{}%", 100 * t.cursor / (n - 1))
    };
    let i = if n == 0 { 0 } else { t.cursor + 1 };
    right.extend(pill(&format!("{pos}  {i}/{n}"), mode_col));

    let center: Vec<Span> = if let Some(p) = app.pending {
        vec![Span::styled(format!("{p}… {}", prefix_desc(p)), bold(YELLOW))]
    } else {
        let k = app.transfers.0.lock().unwrap().tasks.len();
        if k > 0 { vec![Span::styled(format!("⇅ {k} transfert(s) — w : détails"), dim())] } else { vec![] }
    };

    let w = area.width as usize;
    let rw: usize = right.iter().map(|s| s.content.width()).sum::<usize>().min(w);
    let lw: usize = left.iter().map(|s| s.content.width()).sum::<usize>().min(w.saturating_sub(rw));
    let la = Rect { width: lw as u16, ..area };
    let ra = Rect { x: area.x + (w - rw) as u16, width: rw as u16, ..area };
    let ca = Rect { x: area.x + lw as u16, width: (w - lw - rw) as u16, ..area };
    f.render_widget(Line::from(truncate_spans(left, lw)), la);
    f.render_widget(Line::from(truncate_spans(right, rw)), ra);
    if !center.is_empty() && ca.width > 2 {
        let cw = ca.width as usize - 2;
        let spans = truncate_spans(center, cw);
        f.render_widget(Line::from(spans).centered(), Rect { x: ca.x + 1, width: cw as u16, ..ca });
    }
}

pub fn draw_toasts(f: &mut Frame, app: &App) {
    let full = f.area();
    let w = 60.min(full.width / 2).max(20.min(full.width));
    let mut bottom = full.height.saturating_sub(1);
    for t in app.toasts.iter().rev() {
        let col = match t.sev {
            Severity::Info => GREEN,
            Severity::Warning => YELLOW,
            Severity::Error => RED,
        };
        let mut lines = Vec::new();
        if let Some(title) = &t.title {
            lines.push(Line::from(Span::styled(title.clone(), bold(col))));
        }
        for l in t.msg.lines() {
            lines.push(markup(l));
        }
        let inner_w = (w as usize).saturating_sub(4).max(1);
        let h: usize = lines.iter().map(|l| l.width().max(1).div_ceil(inner_w)).sum::<usize>() + 2;
        let h = h as u16;
        if bottom < h + 1 {
            break;
        }
        let area = Rect { x: full.width - w - 1, y: bottom - h, width: w, height: h };
        f.render_widget(Clear, area);
        let block = Block::default()
            .borders(Borders::LEFT)
            .border_type(BorderType::Thick)
            .border_style(fg(col))
            .style(Style::default().bg(surface()))
            .padding(Padding::new(1, 1, 1, 1));
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        bottom = bottom - h - 1;
    }
}
