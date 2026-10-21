//! Lancement de sous-processus avec délai et annulation.

use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
}

/// Exécute `argv`, capture les sorties. `Err` si la commande est absente ou le délai dépassé.
pub fn run(argv: &[String], timeout: Option<Duration>, cancel: Option<&Arc<AtomicBool>>) -> io::Result<Out> {
    run_full(argv, None, &[], timeout, cancel)
}

/// Comme `run`, avec un texte envoyé sur l'entrée standard et des variables d'environnement.
pub fn run_full(
    argv: &[String],
    input: Option<&str>,
    env: &[(&str, &str)],
    timeout: Option<Duration>,
    cancel: Option<&Arc<AtomicBool>>,
) -> io::Result<Out> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .envs(env.iter().copied())
        .process_group(0);
    let mut child = cmd.spawn()?;
    if let (Some(text), Some(mut si)) = (input, child.stdin.take()) {
        let text = format!("{text}\n");
        std::thread::spawn(move || {
            use std::io::Write;
            let _ = si.write_all(text.as_bytes());
        });
    }
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let t1 = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = so.read_to_end(&mut v);
        v
    });
    let t2 = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = se.read_to_end(&mut v);
        v
    });
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        let expired = timeout.map(|t| start.elapsed() > t).unwrap_or(false);
        let cancelled = cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false);
        if expired || cancelled {
            let _ = child.kill();
            let _ = child.wait();
            // pas de join des lecteurs : un `ssh` passé par une connexion maîtresse lui a confié
            // ses sorties, qui peuvent rester ouvertes longtemps après sa mort (maîtresse bloquée) ;
            // les fils de lecture finiront seuls
            drop((t1, t2));
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                if expired { "délai dépassé" } else { "annulé" },
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = String::from_utf8_lossy(&t1.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&t2.join().unwrap_or_default()).into_owned();
    Ok(Out { code: status.code().unwrap_or(-1), stdout, stderr })
}

/// Lance `argv` pendant `dur` puis l'arrête, et renvoie ce qu'il a écrit jusque-là
/// (pour les commandes qui ne se terminent pas seules, comme `dns-sd -B`).
pub fn run_for(argv: &[String], dur: Duration) -> String {
    let Ok(mut child) = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
    else {
        return String::new();
    };
    let mut so = child.stdout.take().unwrap();
    let t = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = so.read_to_end(&mut v);
        v
    });
    let start = Instant::now();
    while start.elapsed() < dur {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    String::from_utf8_lossy(&t.join().unwrap_or_default()).into_owned()
}

pub fn which(name: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|d| !d.is_empty() && std::path::Path::new(d).join(name).is_file())
}

/// Lance un programme détaché (nouvelle session, E/S nulles).
pub fn spawn_detached(argv: &[String]) -> io::Result<()> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
