//! Ctrl+G / `:gui` : passer à l'interface graphique en gardant les onglets ; reprise d'une
//! session venue de l'interface graphique (`--session`).

use crate::app::{App, Msg, Severity};
use crate::session::{connect_all, sibling, Session};
use std::sync::Arc;

impl App {
    pub fn to_gui(&mut self) {
        if self.has_transfers() {
            self.warn("Des transferts sont en cours : attends qu'ils finissent avant de changer d'interface.");
            return;
        }
        let mut s = Session::default();
        for (i, t) in self.tabs.iter().enumerate() {
            if i == self.active {
                s.active = s.tabs.len();
            }
            s.push(&t.fs, &t.cwd, t.current().map(|e| e.name.clone()));
        }
        s.active = s.active.min(s.tabs.len().saturating_sub(1));
        let r = s.save().and_then(|p| {
            let argv = vec![sibling("mysend-gui"), "--session".to_string(), p.to_string_lossy().into_owned()];
            crate::proc::spawn_detached(&argv)
        });
        match r {
            Ok(()) => self.quit = true,
            Err(e) => self.error(&format!("Interface graphique impossible à lancer : {e}")),
        }
    }

    /// Rouvre les onglets d'une session (les machines distantes sont reconnectées d'abord).
    pub fn restore_session(&mut self, s: Session) {
        if s.skipped > 0 {
            self.notify(&format!("{} onglet(s) sudo non repris (mot de passe à redonner).", s.skipped), Severity::Warning, 6.0);
        }
        let targets = s.targets();
        if targets.is_empty() {
            self.apply_session(s, Vec::new(), None);
            return;
        }
        // onglet provisoire le temps des connexions (l'affichage en veut au moins un)
        let home = self.local.home.clone();
        self.add_tab(self.local.clone(), home, None);
        let placeholder = self.tab().id;
        self.notify("Reprise des onglets…", Severity::Info, 3.0);
        self.send_thread(move || {
            let conns = connect_all(targets);
            Box::new(move |app: &mut App| app.apply_session(s, conns, Some(placeholder))) as Msg
        });
    }

    fn apply_session(&mut self, s: Session, conns: Vec<(String, Result<crate::fs::Fs, String>)>, placeholder: Option<u64>) {
        let before = self.tabs.len();
        for (t, r) in conns {
            match r {
                Ok(fs) => {
                    self.remotes.entry(t).or_insert_with(|| Arc::new(fs));
                }
                Err(e) => self.notify_t(&e, &t, Severity::Error, 8.0),
            }
        }
        let mut active = None;
        for (i, t) in s.tabs.into_iter().enumerate() {
            let fs = match &t.target {
                None => Some(self.local.clone()),
                Some(x) => self.remotes.get(x).cloned(),
            };
            let Some(fs) = fs else { continue };
            self.add_tab(fs, t.path, t.select.clone());
            if i == s.active {
                active = Some((self.tabs.len() - 1, t.select));
            }
        }
        // l'onglet provisoire laisse la place aux onglets repris
        if let Some(id) = placeholder {
            if self.tabs.len() > before {
                if let Some(pos) = self.tabs.iter().position(|t| t.id == id) {
                    self.tabs.remove(pos);
                    active = active.map(|(i, sel)| (if i > pos { i - 1 } else { i }, sel));
                }
            }
        }
        if self.tabs.is_empty() {
            let home = self.local.home.clone();
            self.add_tab(self.local.clone(), home, None);
        }
        let (i, sel) = active.unwrap_or((self.active.min(self.tabs.len() - 1), None));
        self.active = i;
        let cwd = self.tab().cwd.clone();
        self.load(cwd, sel);
    }
}
