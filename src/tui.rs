//! Terminal : entrée/sortie du mode plein écran, suspension pour l'éditeur ou un shell.

use crate::app::App;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use std::io::stdout;

pub fn enter() {
    let _ = enable_raw_mode();
    let _ = execute!(stdout(), EnterAlternateScreen, EnableMouseCapture, cursor::Hide);
    // protocole clavier kitty : distingue Ctrl+Tab de Tab (ignoré par les autres terminaux)
    let _ = execute!(stdout(), PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
}

pub fn leave() {
    let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    let _ = execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen, cursor::Show);
    let _ = disable_raw_mode();
}

/// Jette ce qui attend en entrée : octets du terminal et événements déjà lus par crossterm
/// (sinon une touche répétée ou la fin d'une séquence d'échappement ressort après coup).
fn discard_input() {
    unsafe {
        libc::tcflush(libc::STDIN_FILENO, libc::TCIFLUSH);
    }
    while crossterm::event::poll(std::time::Duration::ZERO).unwrap_or(false) {
        let _ = crossterm::event::read();
    }
}

/// Rend le terminal, exécute `f`, puis restaure l'interface.
pub fn suspend<R>(app: &mut App, f: impl FnOnce() -> R) -> R {
    discard_input();
    leave();
    let r = f();
    enter();
    std::thread::sleep(std::time::Duration::from_millis(30));
    discard_input();
    app.needs_clear = true;
    app.preview.image = None;
    r
}
