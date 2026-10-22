//! AirDrop : envoyer des fichiers à un appareil Apple proche.
//!
//! - **Linux** : par `omdrop` (radar de mySend pour choisir l'appareil).
//! - **macOS** : par le panneau AirDrop du système (`native_task`), qui choisit lui-même
//!   l'appareil ; à défaut, la fenêtre AirDrop du Finder avec les fichiers à glisser.
//!
//! Linux, en détail :
//! Même façon de faire que le panneau quickshell : la radio est allumée au besoin
//! (`omdrop on`), les appareils sont relevés toutes les 2 s (`peers --json`), leurs noms
//! demandés en continu (`peers --json -n --stream`), et un « réveilleur » Bluetooth annonce
//! cette machine pour qu'un iPhone au repos allume son AWDL.

use crate::fs::{Entry, FsRef};
use crate::transfers::Task;
use crate::util::basename;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const CLI: &str = "omdrop";
const WAKER: &str = "/run/current-system/sw/lib/omdrop/ble-airdrop-adv.py";

pub fn available() -> bool {
    // macOS : AirDrop est fourni par le système (`native_task`)
    cfg!(target_os = "macos") || crate::proc::which(CLI)
}

/// Vrai si l'envoi passe par le panneau AirDrop du système (macOS) plutôt que par le radar.
pub fn native() -> bool {
    cfg!(target_os = "macos")
}

/// Pourquoi AirDrop n'est pas disponible (message pour l'utilisateur).
/// (Linux seulement : sous macOS, AirDrop est toujours disponible.)
pub fn unavailable_reason() -> &'static str {
    "omdrop introuvable : il faut le module omdrop de ton NixOS."
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub receiving: bool,
    /// La radio annonce (les appareils peuvent être entendus).
    pub visible: bool,
    pub name: String,
}

fn json(argv: &[&str]) -> Option<serde_json::Value> {
    let argv: Vec<String> = std::iter::once(CLI).chain(argv.iter().copied()).map(String::from).collect();
    let o = crate::proc::run(&argv, Some(Duration::from_secs(20)), None).ok()?;
    serde_json::from_str(o.stdout.trim()).ok()
}

pub fn status() -> Option<Status> {
    let v = json(&["status", "--json"])?;
    Some(Status {
        receiving: v["receiving"].as_bool().unwrap_or(false),
        visible: v["visible"].as_bool().unwrap_or(false),
        name: v["name"].as_str().unwrap_or("").to_string(),
    })
}

/// Allume AirDrop pour 10 minutes (le temps de chercher et d'envoyer).
pub fn turn_on() -> Result<(), String> {
    let argv: Vec<String> = [CLI, "on", "10m"].iter().map(|s| s.to_string()).collect();
    let o = crate::proc::run(&argv, Some(Duration::from_secs(60)), None).map_err(|e| e.to_string())?;
    if o.ok() { Ok(()) } else { Err(format!("{}{}", o.stdout.trim(), o.stderr.trim())) }
}

/// Éteint AirDrop (le Wi-Fi reprend la main). omdrop refuse tant qu'un envoi est en cours.
pub fn turn_off() -> Result<(), String> {
    let argv: Vec<String> = [CLI, "off"].iter().map(|s| s.to_string()).collect();
    let o = crate::proc::run(&argv, Some(Duration::from_secs(30)), None).map_err(|e| e.to_string())?;
    if o.ok() { Ok(()) } else { Err(format!("{}{}", o.stdout.trim(), o.stderr.trim())) }
}

#[derive(Clone, Debug)]
pub struct Peer {
    pub mac: String,
    /// Force du signal (dBm), si connue.
    pub rssi: Option<i64>,
    pub name: String,
}

/// Appareils entendus en ce moment.
pub fn peers() -> Vec<Peer> {
    let Some(serde_json::Value::Array(v)) = json(&["peers", "--json"]) else { return Vec::new() };
    v.iter()
        .filter_map(|p| {
            Some(Peer { mac: p["mac"].as_str()?.to_string(), rssi: p["rssi"].as_i64(), name: p["name"].as_str().unwrap_or("").to_string() })
        })
        .collect()
}

/// Lance la recherche des noms ; `found(mac, nom)` pour chaque réponse (nom vide : muet).
/// Renvoie le processus (à arrêter) ; la lecture se fait dans un thread.
pub fn spawn_names(found: impl Fn(String, String) + Send + 'static) -> Option<Child> {
    let mut child = Command::new(CLI)
        .args(["peers", "--json", "-n", "--stream"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let out = child.stdout.take()?;
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                if let Some(mac) = v["mac"].as_str() {
                    found(mac.to_string(), v["name"].as_str().unwrap_or("").to_string());
                }
            }
        }
    });
    Some(child)
}

/// Le réveilleur Bluetooth (5 minutes au plus) ; à arrêter par SIGTERM (il retire son annonce).
pub fn spawn_waker() -> Option<Child> {
    // MYSEND_OMDROP_WAKER : un autre réveilleur (tests : sans annonce Bluetooth)
    let waker = std::env::var("MYSEND_OMDROP_WAKER").unwrap_or_else(|_| WAKER.to_string());
    if !std::path::Path::new(&waker).exists() {
        return None;
    }
    Command::new(&waker)
        .args(["--arm", "A", "--seconds", "300", "--interval-ms", "100"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

/// Arrête proprement un processus lancé ici.
pub fn stop(child: &mut Child) {
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let _ = child.wait();
}

/// Destinataire : un appareil entendu (adresse), ou un nom à chercher.
#[derive(Clone, Debug)]
pub enum Target {
    Mac { mac: String, name: String },
    Name(String),
}

impl Target {
    pub fn label(&self) -> String {
        match self {
            Target::Mac { mac, name } => if name.is_empty() { mac.clone() } else { name.clone() },
            Target::Name(n) => n.clone(),
        }
    }
}

fn last_file() -> std::path::PathBuf {
    crate::config::config_dir().join("airdrop-last")
}

/// Nom du dernier appareil auquel on a envoyé (les adresses AWDL changent, pas les noms).
pub fn last_device() -> Option<String> {
    std::fs::read_to_string(last_file()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn remember(name: &str) {
    if name.is_empty() {
        return;
    }
    let _ = std::fs::create_dir_all(crate::config::config_dir());
    let _ = std::fs::write(last_file(), format!("{name}\n"));
}

/// Prépare les fichiers à envoyer dans `tmp` : les distants sont rapatriés, les dossiers
/// zippés si `zip_dirs` (omdrop n'envoie que des fichiers ; l'AirDrop de macOS prend les
/// dossiers tels quels). Renvoie (chemins locaux, erreurs).
fn gather(ctx: &crate::transfers::Ctx, fs: &FsRef, items: &[(String, Entry)], tmp: &std::path::Path, zip_dirs: bool) -> (Vec<String>, Vec<String>) {
    let mut files: Vec<String> = Vec::new();
    let mut errs = Vec::new();
    for (path, e) in items {
        if ctx.cancelled() {
            break;
        }
        let name = basename(path).to_string();
        // une copie locale si besoin (fichier distant, dossier à zipper)
        let local = if fs.remote || fs.sudo.is_some() {
            ctx.set_label(&format!("Rapatriement de {name}"));
            let l = tmp.join(&name).to_string_lossy().into_owned();
            let local_fs = crate::fs::Fs::local();
            if let Err(err) = crate::transfers::copy(ctx, fs, path, e.is_tree(), &local_fs, &l, &[], &name) {
                errs.push(format!("{name} : {err}"));
                continue;
            }
            l
        } else {
            path.clone()
        };
        if e.is_dir && zip_dirs {
            ctx.set_label(&format!("Compression de {name}"));
            let zip = tmp.join(format!("{name}.zip")).to_string_lossy().into_owned();
            let parent = crate::util::dirname(&local);
            let argv: Vec<String> = ["sh", "-c", "cd \"$1\" && zip -qry \"$2\" \"$3\"", "sh", &parent, &zip, &name].iter().map(|s| s.to_string()).collect();
            match crate::proc::run(&argv, None, None) {
                Ok(o) if o.ok() => files.push(zip),
                Ok(o) => errs.push(format!("{name} : zip : {}", o.stderr.trim())),
                Err(err) => errs.push(format!("{name} : zip : {err}")),
            }
        } else {
            files.push(local);
        }
    }
    (files, errs)
}

/// Tâche d'envoi : les fichiers distants sont d'abord rapatriés, les dossiers zippés.
/// `turn_off` : AirDrop est éteint à la fin (réussite ou non), pour rendre le Wi-Fi.
pub fn send_task(fs: FsRef, items: Vec<(String, Entry)>, target: Target, turn_off: bool) -> Task {
    let n = items.len();
    let who = target.label();
    Task {
        title: format!("AirDrop : {n} élément(s) → {who}"),
        touches: Vec::new(),
        run: Box::new(move |ctx| {
            let tmp = crate::config::mkdtemp("airdrop-").map_err(|e| vec![e.to_string()])?;
            let (files, mut errs) = gather(ctx, &fs, &items, &tmp, true);
            let r = if files.is_empty() || ctx.cancelled() {
                Ok(String::new())
            } else {
                let mut argv: Vec<String> = vec![CLI.into(), "send".into()];
                match &target {
                    Target::Mac { mac, .. } => argv.extend(["--to".to_string(), mac.clone()]),
                    Target::Name(n) => argv.push(n.clone()),
                }
                argv.extend(files.iter().cloned());
                ctx.run_input(&argv, None, &format!("Envoi vers {who} (à accepter sur l'appareil)"))
            };
            let _ = std::fs::remove_dir_all(&tmp);
            if turn_off {
                ctx.set_label("Arrêt d'AirDrop (le Wi-Fi reprend)");
                if let Err(e) = self::turn_off() {
                    errs.push(format!("AirDrop toujours allumé : {e}"));
                }
            }
            match r {
                Ok(_) if errs.is_empty() => {
                    match &target {
                        Target::Mac { name, .. } => remember(name),
                        Target::Name(n) => remember(n),
                    }
                    Ok(Vec::new())
                }
                Ok(_) => Err(errs),
                Err(e) => {
                    errs.push(e);
                    Err(errs)
                }
            }
        }),
    }
}

// ---------------------------------------------------------------- macOS : AirDrop du système

/// Script JXA (JavaScript for Automation, fourni par macOS) : ouvre le panneau AirDrop du
/// système avec les fichiers donnés en arguments et attend la fin de l'envoi.
/// Écrit sur stdout `OK`, `ANNULÉ` ou `ERR <message>` (`ERR` seul : panneau impossible à ouvrir).
const AIRDROP_JXA: &str = r#"
ObjC.import('AppKit');
var state = 0, msg = '';
ObjC.registerSubclass({
  name: 'MySendAirDropDelegate',
  protocols: ['NSSharingServiceDelegate'],
  methods: {
    'sharingService:didShareItems:': {
      types: ['void', ['id', 'id']],
      implementation: function (svc, items) { state = 1; }
    },
    'sharingService:didFailToShareItems:error:': {
      types: ['void', ['id', 'id', 'id']],
      implementation: function (svc, items, err) {
        // 3072 : NSUserCancelledError (fenêtre fermée sans choisir d'appareil)
        state = err.code == 3072 ? 3 : 2;
        msg = ObjC.unwrap(err.localizedDescription);
      }
    }
  }
});
// constantes d'AppKit, avec leur valeur en repli si le pont JXA ne les expose pas
function k(v, dflt) { return v === undefined ? dflt : v; }
function run(argv) {
  var ANY = k($.NSEventMaskAny, Number.MAX_SAFE_INTEGER);
  var MODE = k($.NSDefaultRunLoopMode, 'kCFRunLoopDefaultMode');
  var app = $.NSApplication.sharedApplication;
  app.setActivationPolicy(k($.NSApplicationActivationPolicyRegular, 0));
  app.activateIgnoringOtherApps(true);
  var urls = $.NSMutableArray.array;
  argv.forEach(function (p) { urls.addObject($.NSURL.fileURLWithPath(p)); });
  var svc = $.NSSharingService.sharingServiceNamed('com.apple.share.AirDrop.send');
  if (!svc || svc.isNil() || !svc.canPerformWithItems(urls)) { return 'ERR'; }
  svc.delegate = $.MySendAirDropDelegate.alloc.init;
  svc.performWithItems(urls);
  // boucle d'événements à la main : le panneau en a besoin pour s'afficher et répondre
  while (state === 0) {
    var ev = app.nextEventMatchingMaskUntilDateInModeDequeue(
      ANY, $.NSDate.dateWithTimeIntervalSinceNow(0.2), MODE, true);
    if (ev && !ev.isNil()) { app.sendEvent(ev); }
  }
  return state === 1 ? 'OK' : state === 3 ? 'ANNULÉ' : 'ERR ' + msg;
}
"#;

/// Dernier recours : la fenêtre AirDrop du Finder, et les fichiers montrés dans le Finder
/// (à glisser sur l'appareil).
fn finder_fallback(files: &[String]) {
    let _ = crate::proc::spawn_detached(&["open".into(), "-b".into(), "com.apple.finder.Open-AirDrop".into()]);
    let mut argv = vec!["open".to_string(), "-R".to_string()];
    argv.extend(files.iter().cloned());
    let _ = crate::proc::spawn_detached(&argv);
}

/// Tâche d'envoi par l'AirDrop du système (macOS) : fichiers distants rapatriés, puis panneau
/// AirDrop de macOS, où l'on choisit l'appareil. Les dossiers partent tels quels.
pub fn native_task(fs: FsRef, items: Vec<(String, Entry)>) -> Task {
    let n = items.len();
    Task {
        title: format!("AirDrop : {n} élément(s)"),
        touches: Vec::new(),
        run: Box::new(move |ctx| {
            let tmp = crate::config::mkdtemp("airdrop-").map_err(|e| vec![e.to_string()])?;
            let (files, mut errs) = gather(ctx, &fs, &items, &tmp, false);
            if !files.is_empty() && !ctx.cancelled() {
                let script = tmp.join("airdrop.js");
                let r = std::fs::write(&script, AIRDROP_JXA).map_err(|e| e.to_string()).and_then(|_| {
                    let mut argv = vec!["osascript".to_string(), "-l".into(), "JavaScript".into(), script.to_string_lossy().into_owned()];
                    argv.extend(files.iter().cloned());
                    ctx.run_input(&argv, None, "Choisis l'appareil dans la fenêtre AirDrop")
                });
                match r.as_deref().map(str::trim) {
                    Ok("OK") | Ok("ANNULÉ") => {}
                    Ok(other) => {
                        // panneau impossible : fenêtre AirDrop du Finder (fichiers gardés : pas de
                        // nettoyage de tmp, on ne sait pas quand ils seront glissés)
                        finder_fallback(&files);
                        let why = other.strip_prefix("ERR").unwrap_or(other).trim();
                        errs.push(if why.is_empty() {
                            "Panneau AirDrop indisponible : glisse les fichiers sur l'appareil dans la fenêtre AirDrop du Finder.".into()
                        } else {
                            format!("AirDrop : {why}")
                        });
                        return Err(errs);
                    }
                    Err(e) => {
                        finder_fallback(&files);
                        errs.push(format!("AirDrop : {e} — fenêtre AirDrop du Finder ouverte à la place."));
                        return Err(errs);
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&tmp);
            if errs.is_empty() { Ok(Vec::new()) } else { Err(errs) }
        }),
    }
}

/// Position d'un appareil sur le radar : angle stable (d'après l'adresse) et distance au
/// centre d'après le signal (0 : tout près, 1 : au bord).
pub fn radar_position(p: &Peer) -> (f64, f64) {
    let h = crate::util::sha1_hex(p.mac.as_bytes());
    let a = u32::from_str_radix(&h[..8], 16).unwrap_or(0) as f64 / u32::MAX as f64 * std::f64::consts::TAU;
    let d = match p.rssi {
        Some(r) => ((-(r as f64) - 35.0) / 60.0).clamp(0.15, 0.95),
        None => 0.6,
    };
    (a, d)
}
