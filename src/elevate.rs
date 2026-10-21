//! Accès refusé : rouvrir un dossier avec d'autres droits, via sudo ou en tant qu'un autre
//! utilisateur ssh (mot de passe saisi dans l'app).

use crate::app::{App, Severity};
use crate::fs::{Fs, FsRef};
use crate::modals::ModalKind;
use crate::auth::{bare, ssh_with_password};
use std::sync::Arc;

impl App {
    pub fn offer_elevation(&mut self, fs: FsRef, path: String) {
        if self.modals.iter().any(|m| matches!(m.kind, ModalKind::Elevate(..))) {
            return;
        }
        self.push_modal(ModalKind::Elevate(fs, path));
    }

    /// Après une erreur « Permission denied » : propose d'autres droits.
    pub fn maybe_offer_elevation(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let (fs, path) = self.last_denied.take().unwrap_or_else(|| (self.tab().fs.clone(), self.tab().cwd.clone()));
        self.offer_elevation(fs, path);
    }

    fn machine_label(fs: &Fs) -> String {
        match &fs.sudo {
            Some(s) => s.host.as_deref().map(|h| bare(h).to_string()).unwrap_or_else(|| "local".into()),
            None if fs.remote => bare(&fs.label).to_string(),
            None => "local".into(),
        }
    }

    /// sudo : utilisateur cible puis mot de passe de son propre compte.
    pub fn elevate_sudo(&mut self, base: FsRef, path: String) {
        let host = base.host().map(String::from);
        let machine = Self::machine_label(&base);
        self.prompt(&format!("sudo sur {machine} — devenir l'utilisateur (vide = root) :"), "root", None, move |app, v| {
            let Some(v) = v else { return };
            let user = if v.trim().is_empty() { "root".to_string() } else { v.trim().to_string() };
            let key = format!("sudo:{user}@{}", host.as_deref().unwrap_or("local"));
            if let Some(fs) = app.remotes.get(&key).cloned() {
                app.add_tab(fs, path, None);
                return;
            }
            let label = format!("{user}@{machine}(sudo)");
            let id = app.prompt(&format!("Mot de passe sudo de ton compte sur {machine} :"), "", None, move |app, pw| {
                let Some(pw) = pw else { return };
                app.notify(&format!("Connexion sudo ({label})…"), Severity::Info, 3.0);
                app.send_thread(move || {
                    let r = Fs::connect_sudo(host, &user, &pw, &label);
                    Box::new(move |app: &mut App| match r {
                        Ok(fs) => {
                            let fs = app.remotes.entry(key).or_insert_with(|| Arc::new(fs)).clone();
                            app.add_tab(fs, path, None);
                        }
                        Err(e) => app.notify_t(&e.to_string(), &label, Severity::Error, 8.0),
                    })
                });
            });
            app.mask_prompt(id);
        });
    }

    /// Autre utilisateur : connexion ssh `user@machine` avec son mot de passe.
    pub fn elevate_user(&mut self, base: FsRef, path: String) {
        let host = base.host().map(|h| bare(h).to_string()).unwrap_or_else(|| "localhost".into());
        let machine = Self::machine_label(&base);
        self.prompt(&format!("Se connecter à {machine} en tant que (utilisateur) :"), "", None, move |app, v| {
            let Some(user) = v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else { return };
            let target = format!("{user}@{host}");
            let label = format!("{user}@{machine}");
            if let Some(fs) = app.remotes.get(&target).cloned() {
                app.add_tab(fs, path, None);
                return;
            }
            let id = app.prompt(&format!("Mot de passe de {label} :"), "", None, move |app, pw| {
                let Some(pw) = pw else { return };
                app.notify(&format!("Connexion à {label}…"), Severity::Info, 3.0);
                app.send_thread(move || {
                    let r = ssh_with_password(&target, &pw).and_then(|_| Fs::connect_remote(&target, &label).map_err(|e| e.to_string()));
                    Box::new(move |app: &mut App| match r {
                        Ok(fs) => {
                            let fs = app.remotes.entry(target).or_insert_with(|| Arc::new(fs)).clone();
                            app.add_tab(fs, path, None);
                        }
                        Err(e) => app.notify_t(&e, &label, Severity::Error, 8.0),
                    })
                });
            });
            app.mask_prompt(id);
        });
    }

    fn mask_prompt(&mut self, id: u64) {
        if let Some(ModalKind::Prompt(p)) = self.modal_mut(id) {
            p.input.mask = true;
        }
    }
}
