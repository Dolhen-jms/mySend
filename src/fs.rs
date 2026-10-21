//! Systèmes de fichiers : local et distant (SFTP), modèle commun.

use crate::config::ssh_batch;
use crate::proc::{self, Out};
use crate::sftp::{self, Attrs, SErr, Sftp};
use crate::util::{basename, join, shell_join, splitext};
use std::collections::{HashMap, HashSet};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub mtime: f64,
    pub mode: u32,
}

impl Entry {
    /// Un vrai dossier (un lien vers un dossier est copié comme lien).
    pub fn is_tree(&self) -> bool {
        self.is_dir && !self.is_link
    }
}

pub type FsRef = Arc<Fs>;

pub struct Fs {
    pub remote: bool,
    pub target: String,
    pub label: String,
    pub home: String,
    pub cache: Mutex<HashMap<String, Vec<Entry>>>,
    conn: Mutex<Option<Sftp>>,
    /// Onglet « sudo » : tout passe par sudo sur la machine (`host`, ou en local).
    pub sudo: Option<Sudo>,
}

#[derive(Clone)]
pub struct Sudo {
    pub user: String,
    pub password: String,
    pub host: Option<String>,
}

/// Lit le mot de passe sur la 1re ligne de stdin, valide sudo, puis lance la commande en tant
/// que `$1` (le reste de stdin lui est transmis). `sftp` → sftp-server, depuis le home de `$1`.
/// Pas d'`exec sudo` (ni implicite : d'où le `exit $?` final) : sans terminal, sudo retient la
/// validation par processus parent.
const SUDO_WRAP: &str = r#"IFS= read -r pw
sudo -k
printf '%s\n' "$pw" | sudo -S -p '' -v 2>/dev/null || { echo "mot de passe sudo refusé (ou pas de droits sudo)" >&2; exit 1; }
unset pw
u=$1; shift
if [ "$1" = sftp ]; then
  s=$(command -v ssh 2>/dev/null) && s=$(dirname "$(readlink -f "$s")")/../libexec/sftp-server
  for p in /usr/lib/openssh/sftp-server /usr/libexec/openssh/sftp-server /usr/lib/ssh/sftp-server \
           /usr/libexec/sftp-server /usr/lib/sftp-server "$s"; do
    [ -x "$p" ] && { sudo -n -H -u "$u" sh -c 'cd 2>/dev/null; exec "$0"' "$p"; exit $?; }
  done
  echo "sftp-server introuvable" >&2; exit 127
fi
sudo -n -H -u "$u" "$@"
exit $?"#;

pub fn same(a: &FsRef, b: &FsRef) -> bool {
    Arc::ptr_eq(a, b)
}

fn err(msg: impl Into<String>) -> io::Error {
    io::Error::other(msg.into())
}

fn exists_err() -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, "existe déjà")
}

fn meta_entry(name: String, m: &std::fs::Metadata, is_link: bool) -> Entry {
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    Entry { name, is_dir: m.is_dir(), is_link, size: m.len(), mtime, mode: m.mode() }
}

fn attrs_entry(name: String, a: &Attrs, is_link: bool) -> Entry {
    Entry {
        name,
        is_dir: a.is_dir(),
        is_link,
        size: a.size.unwrap_or(0),
        mtime: a.mtime.unwrap_or(0) as f64,
        mode: a.perm.unwrap_or(0),
    }
}

/// Message d'erreur façon Python (`[Errno 2] No such file or directory`).
pub fn errstr(e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(n) => {
            let s = e.to_string();
            let msg = s.split(" (os error").next().unwrap_or(&s).to_string();
            format!("[Errno {n}] {msg}")
        }
        None => e.to_string(),
    }
}

impl Fs {
    pub fn local() -> Fs {
        Fs {
            remote: false,
            target: String::new(),
            label: "local".into(),
            home: crate::config::home_dir(),
            cache: Mutex::new(HashMap::new()),
            conn: Mutex::new(None),
            sudo: None,
        }
    }

    /// Onglet passant par sudo (sur `host` via ssh, ou en local) en tant que `user`.
    pub fn connect_sudo(host: Option<String>, user: &str, password: &str, label: &str) -> io::Result<Fs> {
        let target = format!("sudo:{user}@{}", host.as_deref().unwrap_or("local"));
        let mut fs = Fs {
            remote: true,
            target,
            label: label.into(),
            home: String::new(),
            cache: Mutex::new(HashMap::new()),
            conn: Mutex::new(None),
            sudo: Some(Sudo { user: user.into(), password: password.into(), host }),
        };
        // vérifie le mot de passe avant d'ouvrir la session SFTP (message d'erreur lisible)
        let o = fs.run_sh(&["true"])?;
        if !o.ok() {
            let e = o.stderr.trim();
            return Err(err(if e.is_empty() { format!("sudo : code {}", o.code) } else { e.to_string() }));
        }
        let (argv, secret) = fs.sftp_argv();
        let fail = |e: io::Error| err(format!("SFTP indisponible via sudo ({e})"));
        let mut s = Sftp::spawn(&argv, secret.as_deref()).map_err(fail)?;
        fs.home = s.realpath(".").map_err(|e| fail(e.into()))?;
        fs.conn = Mutex::new(Some(s));
        Ok(fs)
    }

    /// Commande qui parle SFTP, et le secret à lui envoyer d'abord.
    fn sftp_argv(&self) -> (Vec<String>, Option<String>) {
        if self.sudo.is_some() {
            return (self.cmd_argv(&["sftp"]), self.secret().map(String::from));
        }
        let mut argv = vec!["ssh".to_string()];
        argv.extend(ssh_batch());
        argv.extend(["-s".to_string(), self.target.clone(), "sftp".to_string()]);
        (argv, None)
    }

    /// Texte à envoyer sur stdin des commandes de cette machine (mot de passe sudo).
    pub fn secret(&self) -> Option<&str> {
        self.sudo.as_ref().map(|s| s.password.as_str())
    }

    /// Machine ssh réellement utilisée (None en local).
    pub fn host(&self) -> Option<&str> {
        match &self.sudo {
            Some(s) => s.host.as_deref(),
            None if self.remote => Some(&self.target),
            None => None,
        }
    }

    pub fn connect_remote(target: &str, label: &str) -> io::Result<Fs> {
        let fail = |e: io::Error| err(format!("SFTP indisponible sur {label} ({e})"));
        let mut s = Sftp::connect(target).map_err(fail)?;
        let home = s.realpath(".").map_err(|e| fail(e.into()))?;
        Ok(Fs {
            remote: true,
            target: target.into(),
            label: label.into(),
            home,
            cache: Mutex::new(HashMap::new()),
            conn: Mutex::new(Some(s)),
            sudo: None,
        })
    }

    pub fn close(&self) {
        if let Ok(mut g) = self.conn.lock() {
            if let Some(mut s) = g.take() {
                s.close_conn();
            }
        }
    }

    fn with<T>(&self, mut f: impl FnMut(&mut Sftp) -> Result<T, SErr>) -> io::Result<T> {
        let mut g = self.conn.lock().unwrap();
        for attempt in 0..2 {
            if g.is_none() {
                let (argv, secret) = self.sftp_argv();
                let s = Sftp::spawn(&argv, secret.as_deref())
                    .map_err(|e| err(format!("SFTP indisponible sur {} ({e})", self.label)))?;
                *g = Some(s);
            }
            match f(g.as_mut().unwrap()) {
                Ok(v) => return Ok(v),
                Err(SErr::Status(e)) => return Err(e),
                Err(SErr::Conn(e)) => {
                    *g = None;
                    if attempt == 1 {
                        return Err(e);
                    }
                }
            }
        }
        unreachable!()
    }

    pub fn pretty(&self, path: &str) -> String {
        let h = &self.home;
        if h != "/" && (path == h || path.starts_with(&format!("{h}/"))) {
            format!("~{}", &path[h.len()..])
        } else {
            path.to_string()
        }
    }

    pub fn spec(&self, path: &str) -> String {
        match self.host() {
            Some(h) => format!("{h}:{path}"),
            None => path.to_string(),
        }
    }

    /// Identifiant pour les ancêtres de fusion.
    pub fn ident(&self) -> &str {
        self.host().unwrap_or("local")
    }

    pub fn listdir(&self, path: &str) -> io::Result<Vec<Entry>> {
        if !self.remote {
            let mut out = Vec::new();
            for de in std::fs::read_dir(path)? {
                let Ok(de) = de else { continue };
                let name = de.file_name().to_string_lossy().into_owned();
                let is_link = de.file_type().map(|t| t.is_symlink()).unwrap_or(false);
                let m = match std::fs::metadata(de.path()) {
                    Ok(m) => m,
                    Err(_) => match std::fs::symlink_metadata(de.path()) {
                        Ok(m) => m,
                        Err(_) => continue,
                    },
                };
                out.push(meta_entry(name, &m, is_link));
            }
            return Ok(out);
        }
        self.with(|s| {
            let items = s.readdir(path)?;
            let mut out = Vec::new();
            for (name, a) in items {
                if a.is_link() {
                    let full = join(path, &name);
                    match s.stat(&full) {
                        Ok(t) => out.push(attrs_entry(name, &t, true)),
                        Err(SErr::Conn(e)) => return Err(SErr::Conn(e)),
                        Err(_) => out.push(attrs_entry(name, &a, true)),
                    }
                } else {
                    out.push(attrs_entry(name, &a, false));
                }
            }
            Ok(out)
        })
    }

    pub fn ls(&self, path: &str) -> io::Result<Vec<Entry>> {
        let v = self.listdir(path)?;
        self.cache.lock().unwrap().insert(path.to_string(), v.clone());
        Ok(v)
    }

    pub fn cached(&self, path: &str) -> Option<Vec<Entry>> {
        self.cache.lock().unwrap().get(path).cloned()
    }

    pub fn clear_cache(&self) {
        self.cache.lock().unwrap().clear();
    }

    pub fn stat(&self, path: &str) -> io::Result<Entry> {
        let name = basename(path).to_string();
        if !self.remote {
            let m = std::fs::metadata(path)?;
            let is_link = std::fs::symlink_metadata(path).map(|m| m.file_type().is_symlink()).unwrap_or(false);
            return Ok(meta_entry(name, &m, is_link));
        }
        let a = self.with(|s| s.stat(path))?;
        Ok(attrs_entry(name, &a, false))
    }

    /// Comme `stat`, sans suivre les liens.
    pub fn lstat(&self, path: &str) -> io::Result<Entry> {
        let name = basename(path).to_string();
        if !self.remote {
            let m = std::fs::symlink_metadata(path)?;
            return Ok(meta_entry(name, &m, m.file_type().is_symlink()));
        }
        let a = self.with(|s| s.lstat(path))?;
        Ok(attrs_entry(name, &a, a.is_link()))
    }

    /// Contenu d'un dossier sans suivre les liens.
    pub fn listdir_raw(&self, path: &str) -> io::Result<Vec<Entry>> {
        if !self.remote {
            let mut out = Vec::new();
            for de in std::fs::read_dir(path)? {
                let de = de?;
                let m = std::fs::symlink_metadata(de.path())?;
                out.push(meta_entry(de.file_name().to_string_lossy().into_owned(), &m, m.file_type().is_symlink()));
            }
            return Ok(out);
        }
        let items = self.with(|s| s.readdir(path))?;
        Ok(items.into_iter().map(|(n, a)| { let l = a.is_link(); attrs_entry(n, &a, l) }).collect())
    }

    pub fn read_link(&self, path: &str) -> io::Result<String> {
        if !self.remote {
            return Ok(std::fs::read_link(path)?.to_string_lossy().into_owned());
        }
        self.with(|s| s.readlink(path))
    }

    pub fn symlink(&self, target: &str, path: &str) -> io::Result<()> {
        if !self.remote {
            return std::os::unix::fs::symlink(target, path);
        }
        self.with(|s| s.symlink(target, path))
    }

    /// Fixe les droits et la date de modification (pas pour un lien).
    pub fn set_meta(&self, path: &str, mtime: f64, mode: u32) -> io::Result<()> {
        if !self.remote {
            use std::os::unix::fs::PermissionsExt;
            let t = UNIX_EPOCH + Duration::from_secs_f64(mtime.max(0.0));
            std::fs::File::open(path)?.set_modified(t)?;
            return std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o7777));
        }
        self.with(|s| s.setstat(path, mode & 0o7777, mtime as u32))
    }

    pub fn exists(&self, path: &str) -> bool {
        if !self.remote {
            return std::fs::symlink_metadata(path).is_ok();
        }
        self.with(|s| s.lstat(path)).is_ok()
    }

    /// Les `n` premiers octets ; `None` si ce n'est pas un fichier ordinaire.
    pub fn read_head(&self, path: &str, n: usize) -> io::Result<Option<Vec<u8>>> {
        if !self.remote {
            let m = std::fs::metadata(path)?;
            if !m.is_file() {
                return Ok(None);
            }
            let mut f = std::fs::File::open(path)?;
            let mut buf = Vec::with_capacity(n.min(1 << 20));
            f.by_ref().take(n as u64).read_to_end(&mut buf)?;
            return Ok(Some(buf));
        }
        self.with(|s| {
            let a = s.stat(path)?;
            if !a.is_reg() {
                return Ok(None);
            }
            let mut buf = Vec::new();
            s.read_file(path, Some(n as u64), &mut |d| {
                buf.extend_from_slice(d);
                Ok(())
            })?;
            Ok(Some(buf))
        })
    }

    pub fn touch(&self, path: &str) -> io::Result<()> {
        if !self.remote {
            std::fs::OpenOptions::new().write(true).create_new(true).mode(0o644).open(path)?;
            return Ok(());
        }
        self.with(|s| s.write_file(path, sftp::O_WRITE | sftp::O_CREAT | sftp::O_EXCL, &mut io::empty()))
    }

    pub fn write_bytes(&self, path: &str, data: &[u8]) -> io::Result<()> {
        if !self.remote {
            return std::fs::write(path, data);
        }
        self.with(|s| s.write_file(path, sftp::O_WRITE | sftp::O_CREAT | sftp::O_TRUNC, &mut &data[..]))
    }

    pub fn makedirs(&self, path: &str) -> io::Result<()> {
        if !self.remote {
            return std::fs::create_dir_all(path);
        }
        self.with(|s| {
            let mut cur = String::new();
            for comp in path.split('/').filter(|c| !c.is_empty()) {
                cur = format!("{cur}/{comp}");
                if let Err(e) = s.mkdir(&cur) {
                    match s.stat(&cur) {
                        Ok(a) if a.is_dir() => {}
                        Err(SErr::Conn(c)) => return Err(SErr::Conn(c)),
                        _ => return Err(e),
                    }
                }
            }
            Ok(())
        })
    }

    /// Télécharge un fichier distant vers un chemin local.
    pub fn get(&self, remote: &str, local: &std::path::Path) -> io::Result<()> {
        if !self.remote {
            std::fs::copy(remote, local)?;
            return Ok(());
        }
        self.with(|s| {
            let mut f = std::fs::File::create(local).map_err(SErr::Status)?;
            s.read_file(remote, None, &mut |d| io::Write::write_all(&mut f, d))
        })
    }

    pub fn put(&self, local: &std::path::Path, remote: &str) -> io::Result<()> {
        if !self.remote {
            std::fs::copy(local, remote)?;
            return Ok(());
        }
        self.with(|s| {
            let mut f = std::fs::File::open(local).map_err(SErr::Status)?;
            s.write_file(remote, sftp::O_WRITE | sftp::O_CREAT | sftp::O_TRUNC, &mut f)
        })
    }

    /// Comme `get`, en signalant chaque morceau reçu (octets).
    pub fn get_with(&self, remote: &str, local: &std::path::Path, progress: &mut dyn FnMut(u64)) -> io::Result<()> {
        if !self.remote {
            std::fs::copy(remote, local)?;
            return Ok(());
        }
        self.with(|s| {
            let mut f = std::fs::File::create(local).map_err(SErr::Status)?;
            s.read_file(remote, None, &mut |d| {
                progress(d.len() as u64);
                io::Write::write_all(&mut f, d)
            })
        })
    }

    /// Comme `put`, en signalant chaque morceau envoyé (octets).
    pub fn put_with(&self, local: &std::path::Path, remote: &str, progress: &mut dyn FnMut(u64)) -> io::Result<()> {
        if !self.remote {
            std::fs::copy(local, remote)?;
            return Ok(());
        }
        struct Counting<'a, R: Read> {
            inner: R,
            cb: &'a mut dyn FnMut(u64),
        }
        impl<R: Read> Read for Counting<'_, R> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let n = self.inner.read(buf)?;
                (self.cb)(n as u64);
                Ok(n)
            }
        }
        self.with(|s| {
            let f = std::fs::File::open(local).map_err(SErr::Status)?;
            let mut c = Counting { inner: f, cb: &mut *progress };
            s.write_file(remote, sftp::O_WRITE | sftp::O_CREAT | sftp::O_TRUNC, &mut c)
        })
    }

    pub fn rename(&self, src: &str, dst: &str) -> io::Result<()> {
        if self.exists(dst) {
            return Err(exists_err());
        }
        if !self.remote {
            return std::fs::rename(src, dst);
        }
        self.with(|s| s.rename(src, dst))
    }

    pub fn move_(&self, src: &str, dst: &str) -> io::Result<()> {
        if self.exists(dst) {
            return Err(exists_err());
        }
        let r = if self.remote { self.with(|s| s.rename(src, dst)) } else { std::fs::rename(src, dst) };
        match r {
            Ok(()) => Ok(()),
            Err(e) if !self.remote && e.raw_os_error() != Some(libc::EXDEV) => Err(e),
            Err(_) => {
                let o = self.run_sh(&["mv", "--", src, dst])?;
                if o.ok() { Ok(()) } else { Err(err(o.stderr.trim().to_string())) }
            }
        }
    }

    pub fn remove(&self, path: &str) -> io::Result<()> {
        if !self.remote {
            let m = std::fs::symlink_metadata(path)?;
            return if m.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) };
        }
        let o = self.run_sh(&["rm", "-rf", "--", path])?;
        if o.ok() {
            Ok(())
        } else if !o.stderr.trim().is_empty() {
            Err(err(o.stderr.trim().to_string()))
        } else {
            Err(err(format!("rm : code {}", o.code)))
        }
    }

    /// argv d'une commande sur la machine (directe en local, via ssh à distance).
    /// Onglet sudo : le mot de passe doit être envoyé sur stdin (cf. `secret`).
    pub fn cmd_argv(&self, args: &[&str]) -> Vec<String> {
        let mut args: Vec<&str> = args.to_vec();
        if let Some(s) = &self.sudo {
            let mut w = vec!["sh", "-c", SUDO_WRAP, "sh", s.user.as_str()];
            w.extend(args);
            args = w;
        }
        let Some(host) = self.host() else {
            return args.iter().map(|s| s.to_string()).collect();
        };
        let mut v = vec!["ssh".to_string()];
        v.extend(ssh_batch());
        v.push(host.to_string());
        v.push(shell_join(&args));
        v
    }

    fn run_sh(&self, args: &[&str]) -> io::Result<Out> {
        proc::run_full(&self.cmd_argv(args), self.secret(), &[], None, None)
    }

    /// argv de `sh -c script sh args…` sur la machine.
    /// Shell interactif dans `path` : local, `ssh -t` à distance, ou en tant que l'utilisateur sudo.
    /// Renvoie la commande et le dossier où la lancer.
    pub fn shell_argv(&self, path: &str) -> (Vec<String>, String) {
        let here = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "/".into());
        if let Some(su) = &self.sudo {
            // sudo redemande le mot de passe dans le terminal
            let inner = vec![
                "sudo".to_string(), "-u".into(), su.user.clone(), "sh".into(), "-c".into(),
                "cd \"$1\" 2>/dev/null; exec \"${SHELL:-/bin/sh}\"".into(), "sh".into(), path.to_string(),
            ];
            match &su.host {
                Some(h) => {
                    let mut v = vec!["ssh".to_string(), "-t".to_string()];
                    v.extend(crate::config::ssh_opts());
                    v.push(h.clone());
                    v.push(shell_join(&inner));
                    (v, here)
                }
                None => (inner, here),
            }
        } else if self.remote {
            let mut v = vec!["ssh".to_string(), "-t".to_string()];
            v.extend(crate::config::ssh_opts());
            v.push(self.target.clone());
            v.push(format!("cd {} && exec \"$SHELL\" -l", crate::util::shell_quote(path)));
            (v, here)
        } else {
            (vec![std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "sh".into())], path.to_string())
        }
    }

    pub fn sh_argv(&self, script: &str, args: &[&str]) -> Vec<String> {
        let mut a = vec!["sh", "-c", script, "sh"];
        a.extend(args);
        self.cmd_argv(&a)
    }

    pub fn sh(&self, script: &str, args: &[&str], timeout: Option<Duration>) -> io::Result<Out> {
        proc::run_full(&self.sh_argv(script, args), self.secret(), &[], timeout, None)
    }

    pub fn unique(&self, path: &str, is_dir: bool) -> String {
        let (root, ext) = if is_dir { (path.to_string(), String::new()) } else { splitext(path) };
        let mut i = 1;
        loop {
            let cand = format!("{root}_{i}{ext}");
            if !self.exists(&cand) {
                return cand;
            }
            i += 1;
        }
    }

    pub fn rename_all(&self, pairs: &[(String, String)]) -> io::Result<()> {
        let dsts: Vec<&String> = pairs.iter().map(|p| &p.1).collect();
        let uniq: HashSet<&String> = dsts.iter().copied().collect();
        if uniq.len() != dsts.len() {
            return Err(err("deux éléments auraient le même nom"));
        }
        let srcs: HashSet<&String> = pairs.iter().map(|p| &p.0).collect();
        for (_, d) in pairs {
            if !srcs.contains(d) && self.exists(d) {
                return Err(err(format!("{} existe déjà", basename(d))));
            }
        }
        let pid = std::process::id();
        let mut tmps = Vec::new();
        for (i, (s, _)) in pairs.iter().enumerate() {
            let t = format!("{s}.mysend-{pid}-{i}");
            self.rename(s, &t)?;
            tmps.push(t);
        }
        for (t, (_, d)) in tmps.iter().zip(pairs) {
            self.rename(t, d)?;
        }
        Ok(())
    }
}
