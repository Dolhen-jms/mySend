//! Terminal intégré : un shell dans un pseudo-terminal, dessiné dans une fenêtre par-dessus l'app.

use crate::app::{App, Msg};
use crate::fs::FsRef;
use crate::modals::ModalKind;
use crate::ui::{centered, dialog};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;
use std::io::{Read, Write};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

pub struct Term {
    pub id: u64,
    pub title: String,
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    size: (u16, u16),
    /// Zone de texte au dernier affichage (pour la souris).
    inner: Rect,
    /// Sélection à la souris : point d'ancrage et point courant (ligne, colonne).
    sel: Option<((u16, u16), (u16, u16))>,
    selecting: bool,
}

impl Drop for Term {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Zone de la fenêtre du terminal (85 % × 80 %).
pub fn area(full: Rect) -> Rect {
    centered(full, full.width * 85 / 100, full.height * 8 / 10)
}

const SCROLLBACK: usize = 2000;

fn pty_size(rows: u16, cols: u16) -> PtySize {
    PtySize { rows: rows.max(1), cols: cols.max(1), pixel_width: 0, pixel_height: 0 }
}

impl Term {
    /// Lance `argv` dans `cwd` ; à la fin du programme, la fenêtre se ferme et `fs` est rafraîchi.
    pub fn spawn(tx: Sender<Msg>, id: u64, argv: &[String], cwd: &str, title: String, fs: FsRef) -> std::io::Result<Term> {
        let err = |e: anyhow_like::E| std::io::Error::other(e.0);
        let (w, h) = crossterm::terminal::size().unwrap_or((80, 24));
        let a = area(Rect::new(0, 0, w, h));
        let (rows, cols) = (a.height.saturating_sub(2), a.width.saturating_sub(4));
        let pair = native_pty_system().openpty(pty_size(rows, cols)).map_err(|e| err(e.into()))?;
        let mut cmd = CommandBuilder::new(&argv[0]);
        cmd.args(&argv[1..]);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(cmd).map_err(|e| err(e.into()))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| err(e.into()))?;
        let writer = pair.master.take_writer().map_err(|e| err(e.into()))?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows.max(1), cols.max(1), SCROLLBACK)));
        let p2 = parser.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => p2.lock().unwrap().process(&buf[..n]),
                }
            }
            let _ = tx.send(Box::new(move |app: &mut App| {
                app.modals.retain(|m| !matches!(&m.kind, ModalKind::Term(t) if t.id == id));
                app.refresh_fs(&[fs]);
            }));
        });
        Ok(Term { id, title, parser, writer, master: pair.master, child, size: (rows, cols), inner: Rect::default(), sel: None, selecting: false })
    }

    pub fn key(&mut self, k: &KeyEvent) {
        self.sel = None;
        let mut parser = self.parser.lock().unwrap();
        parser.set_scrollback(0);
        let app_cursor = parser.screen().application_cursor();
        drop(parser);
        if let Some(b) = encode(k, app_cursor) {
            let _ = self.writer.write_all(&b);
            let _ = self.writer.flush();
        }
    }

    /// Souris : glisser pour sélectionner (texte renvoyé au relâchement, à copier),
    /// molette pour remonter dans l'historique.
    pub fn mouse(&mut self, m: &MouseEvent) -> Option<String> {
        let r = self.inner;
        if r.width == 0 || r.height == 0 {
            return None;
        }
        let inside = m.column >= r.x && m.column < r.x + r.width && m.row >= r.y && m.row < r.y + r.height;
        let pos = (
            m.row.clamp(r.y, r.y + r.height - 1) - r.y,
            m.column.clamp(r.x, r.x + r.width - 1) - r.x,
        );
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) if inside => {
                self.sel = Some((pos, pos));
                self.selecting = true;
            }
            MouseEventKind::Down(_) => self.sel = None,
            MouseEventKind::Drag(MouseButton::Left) if self.selecting => {
                if let Some((a, _)) = self.sel {
                    self.sel = Some((a, pos));
                }
            }
            MouseEventKind::Up(MouseButton::Left) if self.selecting => {
                self.selecting = false;
                let (a, b) = self.sel?;
                if a == b {
                    self.sel = None;
                    return None;
                }
                let (s, e) = if a <= b { (a, b) } else { (b, a) };
                let text = self.parser.lock().unwrap().screen().contents_between(s.0, s.1, e.0, e.1 + 1);
                let text: Vec<&str> = text.lines().map(|l| l.trim_end()).collect();
                return Some(text.join("\n"));
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if inside => {
                let up = matches!(m.kind, MouseEventKind::ScrollUp);
                let mut parser = self.parser.lock().unwrap();
                if parser.screen().alternate_screen() {
                    // programme plein écran (less, man…) : la molette fait défiler le programme
                    drop(parser);
                    let k = KeyEvent::from(if up { KeyCode::Up } else { KeyCode::Down });
                    for _ in 0..3 {
                        self.key(&k);
                    }
                } else {
                    let cur = parser.screen().scrollback();
                    parser.set_scrollback(if up { cur + 3 } else { cur.saturating_sub(3) });
                    self.sel = None;
                }
            }
            _ => {}
        }
        None
    }

    pub fn render(&mut self, f: &mut Frame) {
        let scroll = self.parser.lock().unwrap().screen().scrollback();
        let hint = if scroll > 0 { format!("historique −{scroll}") } else { "exit pour revenir".into() };
        let hint = format!(" — {hint} · glisser = copier");
        let a = area(f.area());
        let room = (a.width as usize).saturating_sub(hint.chars().count() + 6);
        let title = crate::util::truncate(&self.title, room);
        let inner = dialog(f, a, Some(&format!("{title}{hint}")));
        self.inner = inner;
        let (rows, cols) = (inner.height, inner.width);
        if rows == 0 || cols == 0 {
            return;
        }
        let mut parser = self.parser.lock().unwrap();
        if (rows, cols) != self.size {
            self.size = (rows, cols);
            let _ = self.master.resize(pty_size(rows, cols));
            parser.set_size(rows, cols);
        }
        let screen = parser.screen();
        let buf = f.buffer_mut();
        for r in 0..rows {
            for c in 0..cols {
                let Some(cell) = screen.cell(r, c) else { continue };
                if cell.is_wide_continuation() {
                    continue;
                }
                let mut st = Style::default().fg(color(cell.fgcolor())).bg(color(cell.bgcolor()));
                if cell.bold() {
                    st = st.add_modifier(Modifier::BOLD);
                }
                if cell.italic() {
                    st = st.add_modifier(Modifier::ITALIC);
                }
                if cell.underline() {
                    st = st.add_modifier(Modifier::UNDERLINED);
                }
                if cell.inverse() != self.selected(r, c) {
                    st = st.add_modifier(Modifier::REVERSED);
                }
                let s = cell.contents();
                if let Some(bc) = buf.cell_mut((inner.x + c, inner.y + r)) {
                    bc.set_symbol(if s.is_empty() { " " } else { &s }).set_style(st);
                }
            }
        }
        if !screen.hide_cursor() && screen.scrollback() == 0 {
            let (cr, cc) = screen.cursor_position();
            if cr < rows && cc < cols {
                f.set_cursor_position((inner.x + cc, inner.y + cr));
            }
        }
    }
}

impl Term {
    fn selected(&self, r: u16, c: u16) -> bool {
        let Some((a, b)) = self.sel else { return false };
        let (s, e) = if a <= b { (a, b) } else { (b, a) };
        (r, c) >= s && (r, c) <= e
    }
}

fn color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(n) => crate::style::sgr_style(&format!("38;5;{n}")).fg.unwrap_or(Color::Reset),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Touche → octets envoyés au programme (codage xterm).
fn encode(k: &KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let csi = |s: &str| format!("\x1b[{s}").into_bytes();
    let arrow = |ch: char| if app_cursor { format!("\x1bO{ch}").into_bytes() } else { format!("\x1b[{ch}").into_bytes() };
    let mut out = match k.code {
        KeyCode::Char(c) if ctrl => {
            let c = c.to_ascii_lowercase();
            match c {
                'a'..='z' => vec![c as u8 - b'a' + 1],
                ' ' | '@' | '2' => vec![0],
                '[' | '3' => vec![0x1b],
                '\\' | '4' => vec![0x1c],
                ']' | '5' => vec![0x1d],
                '^' | '6' => vec![0x1e],
                '_' | '7' | '-' => vec![0x1f],
                _ => c.to_string().into_bytes(),
            }
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => csi("Z"),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => arrow('A'),
        KeyCode::Down => arrow('B'),
        KeyCode::Right => arrow('C'),
        KeyCode::Left => arrow('D'),
        KeyCode::Home => arrow('H'),
        KeyCode::End => arrow('F'),
        KeyCode::Insert => csi("2~"),
        KeyCode::Delete => csi("3~"),
        KeyCode::PageUp => csi("5~"),
        KeyCode::PageDown => csi("6~"),
        KeyCode::F(n @ 1..=4) => format!("\x1bO{}", (b'P' + n - 1) as char).into_bytes(),
        KeyCode::F(n) => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => return None,
            };
            csi(&format!("{code}~"))
        }
        _ => return None,
    };
    if alt {
        out.insert(0, 0x1b);
    }
    Some(out)
}

/// Conversion des erreurs de portable-pty (anyhow) en texte.
mod anyhow_like {
    pub struct E(pub String);
    impl<T: std::fmt::Display> From<T> for E {
        fn from(e: T) -> E {
            E(e.to_string())
        }
    }
}
