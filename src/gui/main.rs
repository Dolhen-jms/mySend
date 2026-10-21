//! mysend-gui — l'interface graphique (GTK4) de mySend, sur le même moteur que l'interface
//! terminal : machines locales et distantes (ssh/sftp, sudo), transferts, corbeille.

mod airdrop_view;
mod compare_view;
mod dialogs;
mod extras;
mod keys;
mod pane;
mod preview;
mod theme;
mod thumbs;
mod transfers_view;
mod util;
mod window;

use adw::prelude::*;
use gtk::{gio, glib};

const USAGE: &str = "usage: mysend-gui [-H HÔTE]... [DOSSIER]

  -H HÔTE, --host HÔTE  ouvre un onglet sur cet hôte (alias ssh, IP, user@hôte) ; répétable
  DOSSIER               dossier local à ouvrir (défaut : le dossier personnel)
  --send FICHIER...     fichiers à envoyer : copiés dans le presse-papiers, puis choix de la machine
  --airdrop FICHIER...  fichiers à envoyer à un appareil Apple proche (omdrop)";

fn main() -> glib::ExitCode {
    if mysend::auth::askpass_main() {
        return glib::ExitCode::SUCCESS;
    }
    // GTK_THEME désactive la feuille de style de libadwaita : les couleurs viennent du thème de bureau
    // (voir theme.rs). Aucun thread n'existe encore.
    unsafe { std::env::remove_var("GTK_THEME") };
    // Rendu OpenGL sur la carte de l'affichage (l'intégrée sur un portable hybride) : le rendu
    // Vulkan par défaut passe en revue toutes les cartes et réveille la NVIDIA endormie (+1,3 s).
    if std::env::var_os("GSK_RENDERER").is_none() {
        unsafe { std::env::set_var("GSK_RENDERER", "ngl") };
    }
    mysend::config::migrate_old_dirs();
    let mut hosts = Vec::new();
    let mut path = None;
    let mut session = None;
    let mut send: Vec<String> = Vec::new();
    let mut airdrop = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            // --send FICHIERS… : à envoyer (presse-papiers rempli, choix de la machine)
            "--send" | "--airdrop" => {
                airdrop = a == "--airdrop";
                let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
                send.extend(it.by_ref().map(|p| mysend::util::normpath(&mysend::util::join(&cwd, &p))));
            }
            "--session" => session = it.next().and_then(|p| mysend::session::Session::take(std::path::Path::new(&p))),
            "-H" | "--host" => match it.next() {
                Some(h) => hosts.push(h),
                None => {
                    eprintln!("{USAGE}\nmysend-gui: -H attend un hôte");
                    return glib::ExitCode::FAILURE;
                }
            },
            s if s.starts_with("-H") && s.len() > 2 => hosts.push(s[2..].to_string()),
            _ => path = Some(a),
        }
    }
    mysend::log::init("gtk", || {});
    let app = adw::Application::builder().application_id("fr.mysend.Gui").flags(gio::ApplicationFlags::NON_UNIQUE).build();
    app.connect_activate(move |app| window::Win::new(app, hosts.clone(), path.clone(), session.clone(), send.clone(), airdrop));
    let code = app.run_with_args(&[] as &[&str]);
    mysend::log::info("── fin");
    code
}
