//! Notifications du bureau (commande propre au système : cf. `platform::notify_argv`).

/// Envoie une notification ; `urgent` : erreur (reste affichée plus longtemps).
pub fn desktop(summary: &str, body: &str, urgent: bool) {
    let _ = crate::proc::spawn_detached(&crate::platform::notify_argv(summary, body, urgent));
}

/// Fin d'un transfert : notification s'il a duré (10 s ou plus) ou s'il a échoué.
pub fn transfer_done(d: &crate::transfers::Done) {
    match &d.result {
        Err(msg) => desktop(&format!("Échec : {}", d.title), msg, true),
        Ok(()) if d.elapsed.as_secs() >= 10 => {
            let secs = d.elapsed.as_secs();
            desktop("Transfert terminé", &format!("{}\n({} min {:02} s)", d.title, secs / 60, secs % 60), false)
        }
        Ok(()) => {}
    }
}
