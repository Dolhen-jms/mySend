//! Chemins, constantes, options ssh/rsync.

use std::path::PathBuf;
use std::sync::OnceLock;

pub const PREVIEW_BYTES: usize = 32 * 1024;
pub const IMAGE_LIMIT: u64 = 50 * 1024 * 1024;
pub const MERGE_LIMIT: u64 = 2 * 1024 * 1024;
pub const FILE_SYNC_LIMIT: u64 = 2 * 1024 * 1024;
pub const MAX_PARALLEL: usize = 4;
pub const SCROLLOFF: usize = 5;

fn env_or(name: &str, default: &str) -> String {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => v,
        _ => default.to_string(),
    }
}

pub fn home_dir() -> String {
    env_or("HOME", "/")
}

fn xdg(name: &str, default_rel: &str) -> PathBuf {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(home_dir()).join(default_rel),
    }
}

/// L'app s'appelait « sendto » : ses dossiers (marque-pages, ancêtres de synchro, cache) sont
/// renommés au premier lancement.
pub fn migrate_old_dirs() {
    for (var, rel) in [("XDG_CONFIG_HOME", ".config"), ("XDG_CACHE_HOME", ".cache"), ("XDG_STATE_HOME", ".local/state")] {
        let base = xdg(var, rel);
        let (old, new) = (base.join("sendto"), base.join("mysend"));
        if old.is_dir() && !new.exists() {
            let _ = std::fs::rename(&old, &new);
        }
    }
}

pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("mysend")
}

pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join("mysend")
}

pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("mysend")
}

/// Éditeur de texte : `MYSEND_EDITOR`, sinon les variables habituelles (`VISUAL`, `EDITOR`),
/// sinon le premier éditeur installé (helix en tête), sinon `vi` (présent partout).
pub fn editor() -> String {
    for var in ["MYSEND_EDITOR", "VISUAL", "EDITOR"] {
        if let Ok(v) = std::env::var(var) {
            if !v.trim().is_empty() {
                return v;
            }
        }
    }
    ["hx", "helix", "nvim", "vim", "nano"]
        .into_iter()
        .find(|e| crate::proc::which(e))
        .unwrap_or("vi")
        .to_string()
}

/// rsync local : celui livré à côté de mySend s'il y en a un (app macOS : un rsync 3 récent,
/// celui du système étant trop vieux), sinon celui du `PATH`.
pub fn rsync_bin() -> String {
    crate::session::sibling("rsync")
}

/// État du rsync local, vérifié une fois (`rsync --version`).
#[derive(Clone, Debug)]
pub enum Rsync {
    /// rsync 3 ou plus : copies par rsync.
    Ok { path: String, version: String },
    /// Trop vieux pour nos options (rsync 2.x, openrsync de macOS) : copies par SFTP.
    Old { path: String, version: String },
    /// Introuvable : copies par SFTP.
    Missing,
}

impl Rsync {
    pub fn usable(&self) -> bool {
        matches!(self, Rsync::Ok { .. })
    }

    /// Phrase pour l'utilisateur.
    pub fn summary(&self) -> String {
        match self {
            Rsync::Ok { path, version } => format!("rsync {version} trouvé ({path}) : copies par rsync."),
            Rsync::Old { path, version } => format!(
                "rsync trop ancien ({version}, {path}) : copies par SFTP (plus lentes, sans reprise). Installe rsync 3."
            ),
            Rsync::Missing => "rsync introuvable : copies par SFTP (plus lentes, sans reprise). Installe rsync 3.".into(),
        }
    }
}

/// rsync local (celui de `rsync_bin`) : présent ? assez récent ? Détecté au premier appel.
pub fn rsync_status() -> &'static Rsync {
    static R: OnceLock<Rsync> = OnceLock::new();
    R.get_or_init(|| {
        let path = rsync_bin();
        let r = match crate::proc::run(&[path.clone(), "--version".into()], Some(std::time::Duration::from_secs(5)), None) {
            Err(_) => Rsync::Missing,
            Ok(o) => {
                let first = o.stdout.lines().next().unwrap_or("").to_string();
                // « rsync  version 3.2.7  protocol version 31 » ; openrsync : « openrsync: protocol version 29 »
                let version = first
                    .split_whitespace()
                    .skip_while(|w| *w != "version")
                    .nth(1)
                    .unwrap_or("?")
                    .to_string();
                let major: u32 = version.split('.').next().and_then(|m| m.parse().ok()).unwrap_or(0);
                if first.contains("openrsync") || major < 3 {
                    let version = if first.contains("openrsync") { "openrsync".to_string() } else { version };
                    Rsync::Old { path, version }
                } else {
                    Rsync::Ok { path, version }
                }
            }
        };
        crate::log::info(&r.summary());
        r
    })
}

pub fn icons() -> bool {
    static ICONS: OnceLock<bool> = OnceLock::new();
    *ICONS.get_or_init(|| env_or("MYSEND_ICONS", "1") != "0")
}

pub fn ssh_opts() -> Vec<String> {
    let rt = env_or("XDG_RUNTIME_DIR", "/tmp");
    vec![
        "-o".into(),
        "ControlMaster=auto".into(),
        "-o".into(),
        format!("ControlPath={rt}/mysend-%C"),
        "-o".into(),
        "ControlPersist=300".into(),
        // connexion maîtresse morte (veille, changement de réseau) : fermée en ~45 s
        "-o".into(),
        "ServerAliveInterval=15".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
    ]
}

pub fn ssh_batch() -> Vec<String> {
    let mut v = ssh_opts();
    v.extend(
        ["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"]
            .iter()
            .map(|s| s.to_string()),
    );
    v
}

pub const RSYNC_OPTS: &[&str] = &["-a", "--partial", "--no-inc-recursive", "--info=progress2"];

/// Crée un dossier temporaire unique `<base>/<prefix>XXXXXXXX`.
pub fn mkdtemp(prefix: &str) -> std::io::Result<PathBuf> {
    let base = cache_dir();
    std::fs::create_dir_all(&base)?;
    for _ in 0..100 {
        let p = base.join(format!("{prefix}{}", rand_suffix()));
        match std::fs::create_dir(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("mkdtemp"))
}

pub fn rand_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ (std::process::id() as u64) << 32
        ^ COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed).wrapping_mul(0x9E3779B97F4A7C15);
    const CH: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    let mut x = n;
    (0..8)
        .map(|_| {
            let c = CH[(x % CH.len() as u64) as usize] as char;
            x = x / CH.len() as u64 ^ x.rotate_left(17);
            c
        })
        .collect()
}
