//! mySend — gestionnaire de fichiers à la yazi, en local et sur les machines du réseau.

pub use mysend::{airdrop, archive, auth, bookmarks, compare, config, discover, fs, highlight, log, notify, pdf, platform, proc, session, sftp, theme, transfers, trash, undo, util};

mod airdrop_tui;
mod app;
mod elevate;
mod help;
mod handoff;
mod input;
mod modals;
mod ops;
mod preview;
mod style;
mod sync;
// terminal intégré par-dessus l'app : désactivé (`!` ouvre un shell plein écran),
// gardé pour pouvoir le rebrancher dans `App::shell`
#[allow(dead_code)]
mod term;
mod tui;
mod ui;

use app::{App, Msg, Severity, Yank};
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::collections::BTreeMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const USAGE: &str = "usage: mysend [-h] [-H HÔTE] [CHEMIN ...]";

fn help() -> String {
    format!(
        "{USAGE}\n\nmySend — gestionnaire de fichiers à la yazi, en local et sur les machines du réseau.\n\n\
         positional arguments:\n  CHEMIN                fichiers à mettre dans le presse-papiers (copie)\n\n\
         options:\n  -h, --help            show this help message and exit\n  \
         -H HÔTE, --host HÔTE  ouvre un onglet sur cet hôte (alias ssh, IP, user@hôte) ; répétable"
    )
}

fn arg_error(msg: &str) -> ! {
    eprintln!("{USAGE}\nmysend: error: {msg}");
    std::process::exit(2);
}

struct Args {
    session: Option<String>,
    hosts: Vec<String>,
    paths: Vec<String>,
}

fn parse_args() -> Args {
    let mut a = Args { session: None, hosts: Vec::new(), paths: Vec::new() };
    let mut it = std::env::args().skip(1);
    let mut only_paths = false;
    while let Some(x) = it.next() {
        if only_paths {
            a.paths.push(x);
            continue;
        }
        match x.as_str() {
            "--" => only_paths = true,
            "-h" | "--help" => {
                println!("{}", help());
                std::process::exit(0);
            }
            "--session" => a.session = it.next(),
            "-H" | "--host" => match it.next() {
                Some(h) => a.hosts.push(h),
                None => arg_error(&format!("argument -H/--host: expected one argument")),
            },
            s if s.starts_with("--host=") => a.hosts.push(s["--host=".len()..].to_string()),
            s if s.starts_with("-H") && s.len() > 2 => a.hosts.push(s[2..].to_string()),
            s if s.starts_with('-') && s.len() > 1 => arg_error(&format!("unrecognized arguments: {s}")),
            _ => a.paths.push(x),
        }
    }
    a
}

/// Protocole d'image : kitty si le terminal le gère, sinon rendu en caractères.
/// (Pas d'interrogation du terminal : un thread resterait bloqué sur stdin.)
fn image_picker() -> ratatui_image::picker::Picker {
    use ratatui_image::picker::{Picker, ProtocolType};
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0;
    let font = if ok && ws.ws_col > 0 && ws.ws_row > 0 && ws.ws_xpixel > 0 && ws.ws_ypixel > 0 {
        (ws.ws_xpixel / ws.ws_col, ws.ws_ypixel / ws.ws_row)
    } else {
        (10, 20)
    };
    let mut p = Picker::from_fontsize(font);
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let kitty = !env("KITTY_WINDOW_ID").is_empty()
        || env("TERM").contains("kitty")
        || env("TERM").contains("ghostty")
        || matches!(env("TERM_PROGRAM").as_str(), "WezTerm" | "ghostty");
    if kitty && env("TMUX").is_empty() {
        p.set_protocol_type(ProtocolType::Kitty);
    } else {
        p.set_protocol_type(ProtocolType::Halfblocks);
    }
    p
}

fn main() {
    if mysend::auth::askpass_main() {
        return;
    }
    config::migrate_old_dirs();
    let args = parse_args();
    let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "/".into());

    // presse-papiers pré-rempli
    let missing: Vec<&String> = args.paths.iter().filter(|p| std::fs::symlink_metadata(p).is_err()).collect();
    if !missing.is_empty() {
        arg_error(&format!("introuvable : {}", missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let local = fs::Fs::local();
    let mut items = BTreeMap::new();
    for p in &args.paths {
        let abs = util::normpath(&util::join(&cwd, p));
        let mut e = match local.stat(&abs) {
            Ok(e) => e,
            Err(_) => continue,
        };
        e.is_link = std::fs::symlink_metadata(&abs).map(|m| m.file_type().is_symlink()).unwrap_or(false);
        items.insert(abs, e);
    }

    log::init("shell", tui::leave);

    style::load_palette();
    let mut theme_stamp = theme::stamp();
    tui::enter();
    let picker = image_picker();
    let mut terminal = match Terminal::new(CrosstermBackend::new(std::io::stdout())) {
        Ok(t) => t,
        Err(e) => {
            tui::leave();
            eprintln!("mysend: {e}");
            std::process::exit(1);
        }
    };

    let (tx, rx) = mpsc::channel::<Msg>();
    let mut app = App::new(tx, picker);
    let session = args.session.as_deref().and_then(|p| session::Session::take(std::path::Path::new(p)));
    match session {
        Some(s) => app.restore_session(s),
        None => app.add_tab(app.local.clone(), cwd, None),
    }
    for h in &args.hosts {
        app.open_tab(Some(h.clone()), h.clone(), None, None);
    }
    if !items.is_empty() {
        let n = items.len();
        app.yank = Some(Yank { fs: app.local.clone(), items, cut: false });
        app.notify(&format!("{n} élément(s) dans le presse-papiers.\nt : ouvrir une machine, p : coller."), Severity::Info, 8.0);
    }
    app.start_discovery();
    // rsync : toujours noté dans le journal ; signalé à l'écran s'il ne sert pas (copies par SFTP)
    let rs = config::rsync_status();
    if !rs.usable() {
        app.notify(&rs.summary(), Severity::Warning, 10.0);
    }

    let mut last_refresh = Instant::now();
    loop {
        app.expire_toasts();
        app.update_preview(app.areas.preview.height.max(1) as usize);
        if app.needs_clear {
            let _ = terminal.clear();
            app.needs_clear = false;
        }
        let _ = terminal.draw(|f| {
            ui::draw(f, &mut app);
            for i in 0..app.modals.len() {
                modals::render(f, &mut app, i);
            }
            ui::draw_toasts(f, &app);
            style::remap(f.buffer_mut());
        });
        if app.quit {
            break;
        }
        if event::poll(Duration::from_millis(50)).unwrap_or(false) {
            loop {
                match event::read() {
                    Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => app.on_key(k),
                    Ok(Event::Mouse(m)) => app.on_mouse(m),
                    _ => {}
                }
                if app.quit || app.needs_clear || !event::poll(Duration::ZERO).unwrap_or(false) {
                    break;
                }
            }
        }
        while let Ok(m) = rx.try_recv() {
            m(&mut app);
        }
        if last_refresh.elapsed() >= Duration::from_secs(2) {
            last_refresh = Instant::now();
            app.auto_refresh();
            // bascule de thème (theme-set) : nouvelles couleurs
            let s = theme::stamp();
            if s != theme_stamp {
                theme_stamp = s;
                style::load_palette();
                app.needs_clear = true;
            }
        }
    }
    for fs in app.remotes.values() {
        fs.close();
    }
    tui::leave();
    log::info("── fin");
}
