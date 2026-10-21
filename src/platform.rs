//! Ce qui change d'un système à l'autre (Linux / macOS), regroupé ici pour que le reste du
//! code n'ait pas à le savoir : ouvrir avec l'application par défaut, notifications du bureau.
//! (La découverte mDNS, propre à chaque système, est dans `discover.rs`.)

/// Programme qui ouvre un fichier avec l'application par défaut (pour les messages).
#[cfg(target_os = "macos")]
pub const OPEN: &str = "open";
#[cfg(not(target_os = "macos"))]
pub const OPEN: &str = "xdg-open";

/// Commande pour ouvrir `path` avec l'application par défaut.
pub fn open_argv(path: &str) -> Vec<String> {
    vec![OPEN.to_string(), path.to_string()]
}

/// Commande d'une notification du bureau ; `urgent` : erreur (reste affichée plus longtemps).
#[cfg(not(target_os = "macos"))]
pub fn notify_argv(summary: &str, body: &str, urgent: bool) -> Vec<String> {
    // org.freedesktop.Notifications par `busctl` (systemd) : rien à installer
    [
        "busctl",
        "--user",
        "call",
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        "Notify",
        "susssasa{sv}i",
        "mySend",
        "0",
        if urgent { "dialog-error" } else { "emblem-ok-symbolic" },
        summary,
        body,
        "0",
        "1",
        "urgency",
        "y",
        if urgent { "2" } else { "1" },
        if urgent { "15000" } else { "6000" },
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Commande d'une notification du bureau (centre de notifications, par AppleScript).
#[cfg(target_os = "macos")]
pub fn notify_argv(summary: &str, body: &str, _urgent: bool) -> Vec<String> {
    // textes passés en arguments du script : pas d'échappement AppleScript à faire
    [
        "osascript",
        "-e",
        "on run argv",
        "-e",
        "display notification (item 2 of argv) with title \"mySend\" subtitle (item 1 of argv)",
        "-e",
        "end run",
        summary,
        body,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
