//! Passage d'une interface à l'autre (terminal ↔ GTK) : les onglets sont écrits dans un fichier,
//! l'autre interface est lancée avec `--session FICHIER` et les rouvre.
//! Les connexions ssh maîtresses (ControlPersist) sont réutilisées : pas de nouveau mot de passe.

use crate::fs::Fs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessTab {
    /// Cible ssh ; `None` : cette machine.
    pub target: Option<String>,
    pub label: String,
    pub path: String,
    /// Élément sous le curseur.
    pub select: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Session {
    pub tabs: Vec<SessTab>,
    pub active: usize,
    /// Onglets sudo non repris (le mot de passe n'est pas transmis).
    #[serde(default)]
    pub skipped: usize,
}

impl Session {
    /// Ajoute un onglet ; un onglet sudo est compté comme non repris.
    pub fn push(&mut self, fs: &Fs, path: &str, select: Option<String>) {
        if fs.sudo.is_some() {
            self.skipped += 1;
            return;
        }
        let target = fs.remote.then(|| fs.target.clone());
        self.tabs.push(SessTab { target, label: fs.label.clone(), path: path.to_string(), select });
    }

    pub fn save(&self) -> std::io::Result<PathBuf> {
        let dir = crate::config::cache_dir();
        std::fs::create_dir_all(&dir)?;
        let p = dir.join(format!("session-{}.json", crate::config::rand_suffix()));
        std::fs::write(&p, serde_json::to_string(self).map_err(std::io::Error::other)?)?;
        Ok(p)
    }

    /// Lit puis efface le fichier.
    pub fn take(path: &Path) -> Option<Session> {
        let t = std::fs::read_to_string(path).ok();
        let _ = std::fs::remove_file(path);
        serde_json::from_str(&t?).ok()
    }

    /// Cibles distantes distinctes, dans l'ordre.
    pub fn targets(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = Vec::new();
        for t in &self.tabs {
            if let Some(x) = &t.target {
                if !v.iter().any(|(y, _)| y == x) {
                    v.push((x.clone(), t.label.clone()));
                }
            }
        }
        v
    }
}

/// Programme voisin (même dossier que l'exécutable courant), sinon cherché dans le PATH.
pub fn sibling(name: &str) -> String {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(name)))
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string())
}

/// Connecte les machines de la session (dans un thread) : (cible, résultat).
pub fn connect_all(targets: Vec<(String, String)>) -> Vec<(String, Result<Fs, String>)> {
    let handles: Vec<_> = targets
        .into_iter()
        .map(|(t, l)| {
            std::thread::spawn(move || {
                // une connexion maîtresse morte bloquerait SFTP : test d'abord (et réouverture au besoin)
                let _ = crate::auth::check_ssh(&t);
                (t.clone(), Fs::connect_remote(&t, &l).map_err(|e| e.to_string()))
            })
        })
        .collect();
    handles.into_iter().filter_map(|h| h.join().ok()).collect()
}
