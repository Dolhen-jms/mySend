//! Petits outils de l'interface graphique.

use gtk::{gio, glib, prelude::*};
use mysend::fs::Entry;

/// Exécute `work` dans un thread, puis `done` sur le fil de l'interface.
pub fn bg<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(T) + 'static) {
    glib::spawn_future_local(async move {
        if let Ok(v) = gio::spawn_blocking(work).await {
            done(v);
        }
    });
}

/// Exécute `f` sur le fil de l'interface (depuis n'importe quel thread).
pub fn on_main(f: impl FnOnce() + Send + 'static) {
    glib::MainContext::default().invoke(f);
}

pub fn boxed(e: Entry) -> glib::BoxedAnyObject {
    glib::BoxedAnyObject::new(e)
}

pub fn entry_of(obj: &glib::Object) -> Entry {
    obj.downcast_ref::<glib::BoxedAnyObject>().map(|b| b.borrow::<Entry>().clone()).expect("élément de liste")
}

/// Icône du thème pour une entrée (dossier, ou d'après le type deviné par le nom).
pub fn icon_for(e: &Entry) -> gio::Icon {
    if e.is_dir {
        return gio::ThemedIcon::new("folder").upcast();
    }
    let (ct, _) = gio::content_type_guess(Some(e.name.as_str()), None);
    gio::content_type_get_icon(&ct)
}

pub fn type_description(e: &Entry) -> String {
    if e.is_dir {
        return "Dossier".into();
    }
    let (ct, _) = gio::content_type_guess(Some(e.name.as_str()), None);
    gio::content_type_get_description(&ct).to_string()
}

pub fn escape(s: &str) -> String {
    glib::markup_escape_text(s).to_string()
}

/// Terminaux essayés dans l'ordre quand `$TERMINAL` n'est pas posé.
const TERMINALS: &[&str] = &["kitty", "ghostty", "wezterm", "alacritty", "foot", "gnome-terminal", "kgx", "konsole", "xterm"];

/// Terminal de l'utilisateur : `$TERMINAL`, sinon le premier installé de `TERMINALS`.
/// `None` : aucun (macOS sans terminal tiers : on passe alors par Terminal.app).
fn user_terminal() -> Option<String> {
    std::env::var("TERMINAL")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| TERMINALS.iter().find(|t| mysend::proc::which(t)).map(|t| t.to_string()))
}

/// Commande qui ouvre une fenêtre de terminal dans le dossier `cwd` (à lancer depuis `cwd`).
pub fn terminal_here(cwd: &str) -> Vec<String> {
    match user_terminal() {
        Some(t) => vec![t],
        None if cfg!(target_os = "macos") => vec!["open".into(), "-a".into(), "Terminal".into(), cwd.into()],
        None => vec!["xterm".into()],
    }
}

/// Terminal.app (macOS) : lance `argv` dans une nouvelle fenêtre et attend qu'elle ait fini,
/// comme les autres terminaux (`osascript` rend la main tout de suite : un fichier témoin,
/// supprimé par la fenêtre à la fin de la commande, sert de signal).
fn in_terminal_app(argv: &[String]) -> Vec<String> {
    const SCRIPT: &str = r#"f=$(mktemp -t mysend) || exit 1
osascript -e 'on run argv' -e 'tell application "Terminal"' -e 'activate' -e 'do script (item 1 of argv)' \
  -e 'end tell' -e 'end run' "$1; rm -f '$f'; exit" >/dev/null || { rm -f "$f"; exit 1; }
while [ -e "$f" ]; do sleep 0.5; done"#;
    vec!["sh".into(), "-c".into(), SCRIPT.into(), "sh".into(), mysend::util::shell_join(argv)]
}

/// Commande pour lancer `argv` dans une nouvelle fenêtre de terminal ; elle se termine quand
/// la fenêtre est fermée.
pub fn in_terminal(argv: &[String]) -> Vec<String> {
    let Some(term) = user_terminal() else {
        if cfg!(target_os = "macos") {
            return in_terminal_app(argv);
        }
        let mut v = vec!["xterm".to_string(), "-e".to_string()];
        v.extend(argv.iter().cloned());
        return v;
    };
    let mut v = vec![term.clone()];
    match mysend::util::basename(&term) {
        // Ctrl+Tab / Ctrl+Maj+Tab : laissés à mysend (onglets) plutôt qu'à kitty
        "kitty" => v.extend(["-o", "map ctrl+tab", "-o", "map ctrl+shift+tab"].map(String::from)),
        "gnome-terminal" | "kgx" => v.push("--".into()),
        _ => v.push("-e".into()),
    }
    v.extend(argv.iter().cloned());
    v
}
