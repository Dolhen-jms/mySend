//! Authentification : mot de passe ssh via SSH_ASKPASS, détection des refus d'accès.

use std::time::Duration;

/// Variable qui fait fonctionner le binaire comme programme SSH_ASKPASS.
pub const ASKPASS_VAR: &str = "MYSEND_ASKPASS_PW";

/// Mode askpass : ssh nous appelle avec l'invite en argument, on répond le mot de passe.
pub fn askpass_main() -> bool {
    let Ok(pw) = std::env::var(ASKPASS_VAR) else { return false };
    let prompt = std::env::args().nth(1).unwrap_or_default().to_lowercase();
    // ne jamais accepter une clé d'hôte inconnue à la place de l'utilisateur
    if prompt.contains("yes/no") || prompt.contains("fingerprint") {
        println!("no");
    } else {
        println!("{pw}");
    }
    true
}

pub fn is_denied(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("permission denied") || m.contains("permission non accordée")
}

/// Nom de machine sans `user@`.
pub fn bare(host: &str) -> &str {
    host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host)
}

/// Au-delà, une connexion maîtresse qui ne répond pas est jugée morte (une commande passée par
/// une maîtresse vivante répond en moins d'une seconde).
const MASTER_PROBE: Duration = Duration::from_secs(8);

fn ssh_ctl(target: &str, op: &str) -> Vec<String> {
    let mut argv = vec!["ssh".to_string()];
    argv.extend(crate::config::ssh_opts());
    argv.extend(["-O".to_string(), op.to_string(), target.to_string()]);
    argv
}

/// Teste la connexion ssh vers `target` (`ssh target true`, en mode non interactif).
///
/// Si une connexion maîtresse existe déjà mais ne répond plus (liaison coupée par une veille,
/// un changement de réseau ou de VPN, et pas encore détectée par `ServerAliveInterval`), la
/// commande y resterait bloquée : au bout de `MASTER_PROBE`, la maîtresse est fermée et le test
/// refait sur une connexion neuve.
pub fn check_ssh(target: &str) -> std::io::Result<crate::proc::Out> {
    let mut argv = vec!["ssh".to_string()];
    argv.extend(crate::config::ssh_batch());
    argv.extend([target.to_string(), "true".to_string()]);
    // `-O check` ne fait que parler au processus local : rapide, même si la liaison est morte
    let master = crate::proc::run(&ssh_ctl(target, "check"), Some(Duration::from_secs(3)), None).is_ok_and(|o| o.ok());
    if master {
        match crate::proc::run(&argv, Some(MASTER_PROBE), None) {
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                crate::log::info(&format!("{target} : connexion maîtresse sans réponse, fermée et rouverte"));
                let _ = crate::proc::run(&ssh_ctl(target, "exit"), Some(Duration::from_secs(3)), None);
            }
            r => return r,
        }
    }
    crate::proc::run(&argv, Some(Duration::from_secs(60)), None)
}

/// Ouvre la connexion maîtresse ssh avec un mot de passe (les sessions suivantes la réutilisent).
pub fn ssh_with_password(target: &str, pw: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut argv = vec!["ssh".to_string()];
    argv.extend(crate::config::ssh_opts());
    for o in ["PreferredAuthentications=password,keyboard-interactive", "NumberOfPasswordPrompts=1", "ConnectTimeout=10"] {
        argv.extend(["-o".to_string(), o.to_string()]);
    }
    argv.extend([target.to_string(), "true".to_string()]);
    let exe = exe.to_string_lossy().into_owned();
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
    let env = [("SSH_ASKPASS", exe.as_str()), ("SSH_ASKPASS_REQUIRE", "force"), (ASKPASS_VAR, pw), ("DISPLAY", display.as_str())];
    let o = crate::proc::run_full(&argv, None, &env, Some(Duration::from_secs(30)), None).map_err(|e| e.to_string())?;
    if o.ok() {
        return Ok(());
    }
    let e = o.stderr.trim();
    Err(if e.contains("Host key verification failed") {
        format!("{e}\nClé d'hôte inconnue : connecte-toi une fois avec « ssh {target} » pour l'accepter.")
    } else if e.contains("Permission denied") {
        format!("Mot de passe refusé (ou connexion par mot de passe interdite pour cet utilisateur).\n{e}")
    } else if e.is_empty() {
        format!("ssh : code {}", o.code)
    } else {
        e.to_string()
    })
}
