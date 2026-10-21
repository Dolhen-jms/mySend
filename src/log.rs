//! Journaux sur disque, dans `$XDG_STATE_HOME/mysend/` :
//! - `mysend.log` : historique de ce qui se passe (onglets, dossiers, connexions, opérations,
//!   transferts, notifications), commun aux deux interfaces ;
//! - `crash.log` : les plantages (panic), avec la pile d'appels.
//!
//! Suivre en direct : `tail -f ~/.local/state/mysend/mysend.log`.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Au-delà, `mysend.log` devient `mysend.log.1` (au démarrage).
const MAX_SIZE: u64 = 5 * 1024 * 1024;

struct Log {
    file: Option<File>,
    tag: String,
}

static LOG: OnceLock<Mutex<Log>> = OnceLock::new();

pub fn dir() -> PathBuf {
    crate::config::state_dir()
}

pub fn path() -> PathBuf {
    dir().join("mysend.log")
}

pub fn crash_path() -> PathBuf {
    dir().join("crash.log")
}

/// Ouvre le journal et installe l'enregistrement des plantages. `iface` : « shell » ou « gtk ».
/// `on_panic` est appelé avant le gestionnaire par défaut (pour rendre le terminal, par ex.).
pub fn init(iface: &str, on_panic: impl Fn() + Send + Sync + 'static) {
    let _ = std::fs::create_dir_all(dir());
    let p = path();
    if std::fs::metadata(&p).map(|m| m.len() > MAX_SIZE).unwrap_or(false) {
        let _ = std::fs::rename(&p, dir().join("mysend.log.1"));
    }
    let file = OpenOptions::new().create(true).append(true).open(&p).ok();
    let tag = format!("{iface} {}", std::process::id());
    let _ = LOG.set(Mutex::new(Log { file, tag }));
    let args: Vec<String> = std::env::args().skip(1).collect();
    info(&format!("── démarrage mysend {} ({iface}) {}", env!("CARGO_PKG_VERSION"), args.join(" ")));

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        on_panic();
        crash(info);
        default_hook(info);
    }));
}

pub fn info(msg: &str) {
    write("INFO", msg);
}

pub fn warn(msg: &str) {
    write("WARN", msg);
}

pub fn error(msg: &str) {
    write("ERREUR", msg);
}

fn stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Une entrée ; les lignes suivantes d'un message sont indentées.
fn write(level: &str, msg: &str) {
    let Some(log) = LOG.get() else { return };
    let Ok(mut l) = log.lock() else { return };
    let head = format!("{} [{}] {level:<6} ", stamp(), l.tag);
    let mut out = String::new();
    for (i, line) in msg.trim_end().lines().enumerate() {
        if i == 0 {
            out.push_str(&head);
        } else {
            out.push_str(&" ".repeat(head.chars().count()));
        }
        out.push_str(line);
        out.push('\n');
    }
    if let Some(f) = l.file.as_mut() {
        let _ = f.write_all(out.as_bytes());
    }
}

fn crash(info: &std::panic::PanicHookInfo) {
    let thread = std::thread::current().name().unwrap_or("?").to_string();
    let msg = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "?".into());
    let at = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
    let tag = LOG.get().and_then(|l| l.lock().ok().map(|l| l.tag.clone())).unwrap_or_default();
    error(&format!("PLANTAGE (fil {thread}) : {msg}\n  à {at}\n  détails : {}", crash_path().display()));
    let bt = std::backtrace::Backtrace::force_capture();
    let args: Vec<String> = std::env::args().collect();
    let text = format!(
        "══ {} — mysend {} [{tag}]\ncommande : {}\nfil : {thread}\nà : {at}\nmessage : {msg}\n\n{bt}\n\n",
        stamp(),
        env!("CARGO_PKG_VERSION"),
        args.join(" "),
    );
    let _ = std::fs::create_dir_all(dir());
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(crash_path()) {
        let _ = f.write_all(text.as_bytes());
    }
}
