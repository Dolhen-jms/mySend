//! File de transferts : rsync sur ssh, 4 tâches simultanées, progression, annulation.

use crate::config::{mkdtemp, ssh_batch, MAX_PARALLEL, RSYNC_OPTS};
use crate::fs::{errstr, same, Entry, Fs, FsRef};
use crate::util::{basename, human, is_within, join, now_hms, shell_join};
use regex::Regex;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq)]
pub enum LogKind {
    Plain,
    Bold,
    Green,
    Red,
}

#[derive(Clone)]
pub struct Running {
    pub started: Instant,
    pub label: String,
    pub pct: Option<u8>,
    pub extra: String,
}

pub struct TaskInfo {
    pub id: u64,
    pub title: String,
    pub running: Option<Running>,
    pub cancelled: bool,
    pub pid: Option<u32>,
}

#[derive(Default)]
pub struct State {
    pub tasks: Vec<TaskInfo>,
    pub log: Vec<(String, LogKind)>,
    active: usize,
    next_id: u64,
}

pub type Shared = Arc<(Mutex<State>, Condvar)>;

pub fn new_shared() -> Shared {
    Arc::new((Mutex::new(State::default()), Condvar::new()))
}

/// Résultat d'une tâche : lignes de journal, ou erreurs.
pub type TaskResult = Result<Vec<String>, Vec<String>>;

pub struct Task {
    pub title: String,
    pub touches: Vec<FsRef>,
    pub run: Box<dyn FnOnce(&Ctx) -> TaskResult + Send>,
}

/// Fin d'une tâche, transmise à l'interface : `Err` porte un message court (5 lignes au plus).
pub struct Done {
    pub title: String,
    pub result: Result<(), String>,
    pub touches: Vec<FsRef>,
    pub elapsed: std::time::Duration,
    /// Ce que la tâche a fait et qu'on peut annuler (Ctrl+Z).
    pub undo: Vec<crate::undo::UndoOp>,
}

pub struct Ctx {
    id: u64,
    shared: Shared,
    undo: Mutex<Vec<crate::undo::UndoOp>>,
}

fn progress_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^([\d.,' ]+?)\s+(\d+)%\s+(\S+/s)\s+(\d+:\d{2}:\d{2})").unwrap())
}

impl Ctx {
    fn with<T>(&self, f: impl FnOnce(&mut TaskInfo) -> T) -> Option<T> {
        let mut st = self.shared.0.lock().unwrap();
        st.tasks.iter_mut().find(|t| t.id == self.id).map(f)
    }

    /// Retient une opération annulable.
    pub fn record(&self, op: crate::undo::UndoOp) {
        self.undo.lock().unwrap().push(op);
    }

    pub fn cancelled(&self) -> bool {
        self.with(|t| t.cancelled).unwrap_or(true)
    }

    pub fn set_label(&self, label: &str) {
        self.with(|t| {
            if let Some(r) = &mut t.running {
                r.label = label.to_string();
                r.pct = None;
                r.extra.clear();
            }
        });
    }

    pub fn set_pct(&self, pct: u8) {
        self.with(|t| {
            if let Some(r) = &mut t.running {
                r.pct = Some(pct);
            }
        });
    }

    pub fn set_extra(&self, extra: String) {
        self.with(|t| {
            if let Some(r) = &mut t.running {
                r.extra = extra;
            }
        });
    }

    /// Lance une commande (rsync ou script), suit la progression, annulable.
    pub fn run_input(&self, argv: &[String], input: Option<&str>, label: &str) -> Result<String, String> {
        self.exec(argv, input, label).map_err(|e| e.1)
    }

    /// Comme `run`, mais l'erreur garde le code de sortie (127 si la commande est absente).
    fn exec(&self, argv: &[String], input: Option<&str>, label: &str) -> Result<String, (Option<i32>, String)> {
        if self.cancelled() {
            return Err((None, "annulé".into()));
        }
        self.set_label(label);
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = cmd.spawn().map_err(|e| {
            let code = if e.kind() == std::io::ErrorKind::NotFound { Some(127) } else { None };
            (code, format!("{}: {e}", argv[0]))
        })?;
        self.with(|t| t.pid = Some(child.id()));
        if let (Some(text), Some(mut si)) = (input, child.stdin.take()) {
            let text = format!("{text}\n");
            std::thread::spawn(move || {
                use std::io::Write;
                let _ = si.write_all(text.as_bytes());
            });
        }
        let mut se = child.stderr.take().unwrap();
        let errt = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = se.read_to_string(&mut s);
            s
        });
        let mut so = child.stdout.take().unwrap();
        let mut buf = [0u8; 4096];
        let mut line = Vec::new();
        let mut out = String::new();
        loop {
            let n = match so.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for &c in &buf[..n] {
                if c == b'\r' || c == b'\n' {
                    let l = String::from_utf8_lossy(&line).into_owned();
                    self.parse_progress(l.trim());
                    out.push_str(&l);
                    out.push('\n');
                    line.clear();
                } else {
                    line.push(c);
                }
            }
        }
        let status = child.wait();
        self.with(|t| t.pid = None);
        let stderr = errt.join().unwrap_or_default();
        if self.cancelled() {
            return Err((None, "annulé".into()));
        }
        match status {
            Ok(s) if s.success() => Ok(out),
            Ok(s) => {
                let e = stderr.trim();
                let msg = if e.is_empty() { format!("{} : code {}", argv[0], s.code().unwrap_or(-1)) } else { e.to_string() };
                Err((s.code(), msg))
            }
            Err(e) => Err((None, e.to_string())),
        }
    }

    fn parse_progress(&self, l: &str) {
        let Some(c) = progress_re().captures(l) else { return };
        let bytes: u64 = c[1].chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0);
        let pct: u8 = c[2].parse().unwrap_or(0);
        let extra = format!("{}  {}  reste {}", human(bytes), &c[3], &c[4]);
        self.with(|t| {
            if let Some(r) = &mut t.running {
                r.pct = Some(pct.min(100));
                r.extra = extra;
            }
        });
    }
}

/// Met la tâche en file ; `finish` est appelé (sur le fil de la tâche) quand elle se termine.
pub fn enqueue(shared: &Shared, task: Task, finish: impl FnOnce(Done) + Send + 'static) {
    let id = {
        let mut st = shared.0.lock().unwrap();
        st.next_id += 1;
        let id = st.next_id;
        st.tasks.push(TaskInfo { id, title: task.title.clone(), running: None, cancelled: false, pid: None });
        st.log.push((format!("{} En file : {}", now_hms(), task.title), LogKind::Plain));
        crate::log::info(&format!("transfert en file : {}", task.title));
        id
    };
    let shared = shared.clone();
    std::thread::spawn(move || {
        let (lock, cv) = &*shared;
        {
            let mut st = lock.lock().unwrap();
            loop {
                let cancelled = st.tasks.iter().find(|t| t.id == id).map(|t| t.cancelled).unwrap_or(true);
                if cancelled {
                    st.tasks.retain(|t| t.id != id);
                    st.log.push((format!("Annulé : {}", task.title), LogKind::Plain));
                    crate::log::info(&format!("transfert annulé : {}", task.title));
                    return;
                }
                if st.active < MAX_PARALLEL {
                    break;
                }
                st = cv.wait(st).unwrap();
            }
            st.active += 1;
            if let Some(t) = st.tasks.iter_mut().find(|t| t.id == id) {
                t.running = Some(Running { started: Instant::now(), label: task.title.clone(), pct: None, extra: String::new() });
            }
            st.log.push((format!("{} {}", now_hms(), task.title), LogKind::Bold));
            crate::log::info(&format!("transfert démarré : {}", task.title));
        }
        let ctx = Ctx { id, shared: shared.clone(), undo: Mutex::new(Vec::new()) };
        let started = Instant::now();
        let res = (task.run)(&ctx);
        let res = match res {
            Ok(v) if ctx.cancelled() => {
                let _ = v;
                Err(vec!["annulé".to_string()])
            }
            r => r,
        };
        {
            let mut st = lock.lock().unwrap();
            st.active -= 1;
            st.tasks.retain(|t| t.id != id);
            match &res {
                Ok(lines) => {
                    for l in lines {
                        st.log.push((l.clone(), LogKind::Plain));
                    }
                    st.log.push(("  terminé".into(), LogKind::Green));
                    let mut m = format!("transfert terminé ({:.1} s) : {}", started.elapsed().as_secs_f64(), task.title);
                    for l in lines {
                        m.push_str(&format!("\n  {l}"));
                    }
                    crate::log::info(&m);
                }
                Err(errs) => {
                    crate::log::error(&format!("transfert échoué ({:.1} s) : {}\n  {}", started.elapsed().as_secs_f64(), task.title, errs.join("\n  ")));
                    for e in errs {
                        for l in e.lines() {
                            st.log.push((format!("  {l}"), LogKind::Red));
                        }
                    }
                }
            }
            cv.notify_all();
        }
        let result = res.map(|_| ()).map_err(|errs| {
            let lines: Vec<String> = errs.iter().flat_map(|e| e.lines().map(String::from)).collect();
            let mut msg = lines.iter().take(5).cloned().collect::<Vec<_>>().join("\n");
            if lines.len() > 5 {
                msg.push('…');
            }
            msg
        });
        let undo = std::mem::take(&mut *ctx.undo.lock().unwrap());
        finish(Done { title: task.title, result, touches: task.touches, elapsed: started.elapsed(), undo });
    });
}

pub fn cancel(shared: &Shared, id: u64) {
    let (lock, cv) = &**shared;
    let mut st = lock.lock().unwrap();
    if let Some(t) = st.tasks.iter_mut().find(|t| t.id == id) {
        t.cancelled = true;
        if let Some(pid) = t.pid {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
    }
    cv.notify_all();
}

pub fn cancel_all(shared: &Shared) {
    let ids: Vec<u64> = shared.0.lock().unwrap().tasks.iter().map(|t| t.id).collect();
    for id in ids {
        cancel(shared, id);
    }
}

// ---------------------------------------------------------------- rsync

pub fn side(fs: &Fs) -> Option<&str> {
    if fs.remote { Some(&fs.target) } else { None }
}

pub fn rsync_argv(src_t: Option<&str>, src: &str, dst_t: Option<&str>, dst: &str, extra: &[&str]) -> Vec<String> {
    let mut opts: Vec<&str> = RSYNC_OPTS.to_vec();
    opts.extend(extra);
    rsync_argv_with(&opts, src_t, src, dst_t, dst)
}

/// Forme de la commande rsync (§ 15.1) avec des options données.
pub fn rsync_argv_with(opts: &[&str], src_t: Option<&str>, src: &str, dst_t: Option<&str>, dst: &str) -> Vec<String> {
    let opts: Vec<String> = opts.iter().map(|s| s.to_string()).collect();
    if let (Some(a), Some(b)) = (src_t, dst_t) {
        if a == b {
            let mut inner = vec!["rsync".to_string()];
            inner.extend(opts);
            inner.push(src.into());
            inner.push(dst.into());
            let mut v = vec!["ssh".to_string()];
            v.extend(ssh_batch());
            v.push(a.to_string());
            v.push(shell_join(&inner));
            return v;
        }
    }
    let spec = |t: Option<&str>, p: &str| match t {
        Some(t) => format!("{t}:{p}"),
        None => p.to_string(),
    };
    let mut v = vec![crate::config::rsync_bin()];
    v.extend(opts);
    v.push("--protect-args".into());
    v.push("-e".into());
    let mut ssh = vec!["ssh".to_string()];
    ssh.extend(ssh_batch());
    v.push(shell_join(&ssh));
    v.push(spec(src_t, src));
    v.push(spec(dst_t, dst));
    v
}

/// Copie `src` (FS a) vers `dst` (FS b) ; dossier : contenu dans dst. Relais local si deux distants.
pub fn copy(ctx: &Ctx, a: &Fs, src: &str, tree: bool, b: &Fs, dst: &str, extra: &[&str], label: &str) -> Result<(), String> {
    if let (Some(x), Some(y)) = (side(a), side(b)) {
        if x != y {
            let tmp = mkdtemp("relay-").map_err(|e| e.to_string())?;
            let lp = tmp.join(basename(src.trim_end_matches('/'))).to_string_lossy().into_owned();
            let local = Fs::local();
            let r = leg(ctx, a, src, tree, &local, &lp, extra, &format!("{label} (1/2)"))
                .and_then(|_| leg(ctx, &local, &lp, tree, b, dst, extra, &format!("{label} (2/2)")));
            let _ = std::fs::remove_dir_all(&tmp);
            return r;
        }
    }
    leg(ctx, a, src, tree, b, dst, extra, label)
}

/// Une copie rsync ; si rsync manque d'un côté, repli sur une copie SFTP.
fn leg(ctx: &Ctx, a: &Fs, src: &str, tree: bool, b: &Fs, dst: &str, extra: &[&str], label: &str) -> Result<(), String> {
    let slash = |p: &str| format!("{}/", p.trim_end_matches('/'));
    let (s, d) = if tree { (slash(src), slash(dst)) } else { (src.to_string(), dst.to_string()) };
    // un onglet sudo ne passe pas par rsync : copie SFTP directe
    let res = if a.sudo.is_some() || b.sudo.is_some() {
        Err((None, String::new()))
    } else {
        ctx.exec(&rsync_argv(side(a), &s, side(b), &d, extra), None, label)
    };
    let sudo = a.sudo.is_some() || b.sudo.is_some();
    match res {
        Ok(_) => Ok(()),
        Err((code, msg)) if !ctx.cancelled() && (sudo || rsync_missing(code, &msg)) => {
            ctx.set_label(&if sudo { format!("{label} (sftp)") } else { format!("{label} (sftp, rsync absent)") });
            let opts = CopyOpts { update: extra.contains(&"-u"), delete: extra.contains(&"--delete") };
            let r = if tree {
                let mut prog = Progress::new(tree_size(a, src));
                walk(ctx, a, src, b, dst, &opts, &mut prog)
            } else {
                let e = a.lstat(src).map_err(|e| format!("{src} : {}", errstr(&e)))?;
                let mut prog = Progress::new(e.size);
                copy_entry(ctx, a, src, &e, b, dst, &opts, &mut prog)
            };
            if r.is_ok() {
                ctx.set_pct(100);
            }
            r
        }
        Err((_, msg)) => Err(msg),
    }
}

/// rsync inutilisable d'un côté : absent, ou trop vieux pour nos options (rsync 2.6 / openrsync
/// de macOS). Dans les deux cas, la copie se refait par SFTP.
fn rsync_missing(code: Option<i32>, msg: &str) -> bool {
    code == Some(127)
        || [
            // absent
            "remote command not found",
            "command not found",
            "commande introuvable",
            "rsync: not found",
            // trop vieux
            "unknown option",
            "unrecognized option",
            "syntax or usage error",
            "protocol version mismatch",
        ]
        .iter()
        .any(|p| msg.contains(p))
}

// ---------------------------------------------------------------- repli SFTP

struct CopyOpts {
    /// `-u` : ne remplace pas un fichier plus récent à l'arrivée.
    update: bool,
    /// `--delete` : supprime ce qui est en trop à l'arrivée.
    delete: bool,
}

/// Copie récursive du contenu de `src` dans `dst` (comme `rsync -a src/ dst/`).
fn walk(ctx: &Ctx, a: &Fs, src: &str, b: &Fs, dst: &str, o: &CopyOpts, prog: &mut Progress) -> Result<(), String> {
    let err = |p: &str, e: std::io::Error| format!("{p} : {}", errstr(&e));
    match b.lstat(dst) {
        Ok(e) if e.is_dir && !e.is_link => {}
        Ok(_) => {
            b.remove(dst).map_err(|e| err(dst, e))?;
            b.makedirs(dst).map_err(|e| err(dst, e))?;
        }
        Err(_) => b.makedirs(dst).map_err(|e| err(dst, e))?,
    }
    let entries = a.listdir_raw(src).map_err(|e| err(src, e))?;
    if o.delete {
        let names: std::collections::HashSet<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        for e in b.listdir_raw(dst).map_err(|e| err(dst, e))? {
            if !names.contains(e.name.as_str()) {
                let p = join(dst, &e.name);
                b.remove(&p).map_err(|e| err(&p, e))?;
            }
        }
    }
    for e in &entries {
        if ctx.cancelled() {
            return Err("annulé".into());
        }
        let (sp, dp) = (join(src, &e.name), join(dst, &e.name));
        if e.is_dir && !e.is_link {
            walk(ctx, a, &sp, b, &dp, o, prog)?;
        } else {
            copy_entry(ctx, a, &sp, e, b, &dp, o, prog)?;
        }
    }
    if let Ok(st) = a.lstat(src) {
        let _ = b.set_meta(dst, st.mtime, st.mode);
    }
    Ok(())
}

/// Copie un fichier ou un lien (le lien est recréé tel quel).
fn copy_entry(ctx: &Ctx, a: &Fs, src: &str, e: &Entry, b: &Fs, dst: &str, o: &CopyOpts, prog: &mut Progress) -> Result<(), String> {
    let err = |p: &str, e: std::io::Error| format!("{p} : {}", errstr(&e));
    let existing = b.lstat(dst).ok();
    if e.is_link {
        let target = a.read_link(src).map_err(|e| err(src, e))?;
        if let Some(d) = &existing {
            if d.is_link && b.read_link(dst).ok().as_deref() == Some(target.as_str()) {
                return Ok(());
            }
            b.remove(dst).map_err(|e| err(dst, e))?;
        }
        return b.symlink(&target, dst).map_err(|e| err(dst, e));
    }
    if let Some(d) = &existing {
        let (sm, dm) = (e.mtime.floor(), d.mtime.floor());
        let same = !d.is_dir && d.size == e.size && sm == dm;
        if same || (o.update && !d.is_dir && dm > sm) {
            prog.add(ctx, e.size);
            return Ok(());
        }
        if d.is_dir || d.is_link {
            b.remove(dst).map_err(|e| err(dst, e))?;
        }
    }
    let before = prog.done;
    let r = match (a.remote, b.remote) {
        (false, false) => std::fs::copy(src, dst).map(|_| ()),
        (false, true) => b.put_with(std::path::Path::new(src), dst, &mut |n| prog.add(ctx, n)),
        (true, false) => a.get_with(src, std::path::Path::new(dst), &mut |n| prog.add(ctx, n)),
        (true, true) => b
            .sh("cp -p -- \"$1\" \"$2\"", &[src, dst], None)
            .and_then(|o| if o.ok() { Ok(()) } else { Err(std::io::Error::other(o.stderr.trim().to_string())) }),
    };
    r.map_err(|e| err(src, e))?;
    let _ = b.set_meta(dst, e.mtime, e.mode);
    // ce que les morceaux n'ont pas compté (copie locale, cp à distance)
    let counted = prog.done - before;
    prog.add(ctx, e.size.saturating_sub(counted));
    prog.files += 1;
    Ok(())
}

/// Tâche de collage : copie (ou déplace si `cut`) `items` de `sfs` dans `ddir` sur `dfs`.
/// Sans `overwrite`, un nom déjà pris reçoit un suffixe.
pub fn paste_task(title: String, sfs: FsRef, items: Vec<(String, Entry)>, cut: bool, overwrite: bool, dfs: FsRef, ddir: String) -> Task {
    let n = items.len();
    let (a, b) = (sfs.clone(), dfs.clone());
    Task {
        title,
        touches: vec![sfs.clone(), dfs.clone()],
        run: Box::new(move |ctx| {
            let mut errs = Vec::new();
            let same_fs = same(&a, &b);
            for (i, (src, e)) in items.iter().enumerate() {
                if ctx.cancelled() {
                    break;
                }
                let name = basename(src).to_string();
                let label = if n > 1 { format!("{}/{n} {name}", i + 1) } else { name.clone() };
                let mut dst = join(&ddir, &name);
                if same_fs && e.is_dir && is_within(&ddir, src) {
                    errs.push(format!("{name} : on ne peut pas mettre un dossier dans lui-même"));
                    continue;
                }
                if same_fs && dst == *src {
                    if cut || overwrite {
                        continue;
                    }
                    dst = b.unique(&dst, e.is_tree());
                } else if !overwrite && b.exists(&dst) {
                    dst = b.unique(&dst, e.is_tree());
                }
                let fresh = !b.exists(&dst);
                let r = if cut && same_fs && fresh {
                    ctx.set_label(&label);
                    let r = b.move_(src, &dst).map_err(|e| errstr(&e));
                    ctx.set_pct(100);
                    if r.is_ok() {
                        ctx.record(crate::undo::UndoOp::Move { fs: b.clone(), from: src.clone(), to: dst.clone() });
                    }
                    r
                } else {
                    let r = copy(ctx, &a, src, e.is_tree(), &b, &dst, &[], &label);
                    if r.is_ok() && !cut && fresh {
                        ctx.record(crate::undo::UndoOp::Copy { fs: b.clone(), path: dst.clone() });
                    }
                    if r.is_ok() && cut {
                        a.remove(src).map_err(|e| errstr(&e))
                    } else {
                        r
                    }
                };
                if let Err(e) = r {
                    errs.push(format!("{name} : {e}"));
                }
            }
            if errs.is_empty() { Ok(Vec::new()) } else { Err(errs) }
        }),
    }
}

/// Progression d'une copie SFTP : octets faits sur le total, vitesse, temps restant.
pub struct Progress {
    total: u64,
    done: u64,
    files: u64,
    start: Instant,
    shown: Option<Instant>,
}

impl Progress {
    fn new(total: u64) -> Progress {
        Progress { total, done: 0, files: 0, start: Instant::now(), shown: None }
    }

    /// `n` octets de plus ; l'affichage est rafraîchi quatre fois par seconde au plus.
    fn add(&mut self, ctx: &Ctx, n: u64) {
        self.done += n;
        let now = Instant::now();
        if self.shown.map(|t| now.duration_since(t).as_millis() < 250).unwrap_or(false) && self.done < self.total {
            return;
        }
        self.shown = Some(now);
        let secs = self.start.elapsed().as_secs_f64().max(0.001);
        let speed = self.done as f64 / secs;
        let mut s = format!("{} / {}  {}/s", human(self.done), human(self.total), human(speed as u64));
        if speed > 0.0 && self.total > self.done {
            let left = ((self.total - self.done) as f64 / speed) as u64;
            s.push_str(&format!("  reste {}:{:02}:{:02}", left / 3600, left / 60 % 60, left % 60));
        }
        ctx.set_extra(s);
        if self.total > 0 {
            ctx.set_pct(((self.done.min(self.total) * 100) / self.total) as u8);
        }
    }
}

/// Taille totale d'une arborescence (les liens ne sont pas suivis).
fn tree_size(fs: &Fs, path: &str) -> u64 {
    let Ok(v) = fs.listdir(path) else { return 0 };
    v.iter()
        .map(|e| if e.is_tree() { tree_size(fs, &join(path, &e.name)) } else if e.is_link { 0 } else { e.size })
        .sum()
}
