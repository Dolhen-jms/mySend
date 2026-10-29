//! Boîtes de dialogue : confirmation, formulaire (texte, mot de passe), choix de la machine.

use crate::window::win;
use adw::prelude::*;
use gtk::{gdk, glib};
use mysend::discover::Host;

/// Demande confirmation ; `destructive` colore le bouton en rouge.
pub fn confirm(parent: &impl IsA<gtk::Widget>, heading: &str, body: &str, ok: &str, destructive: bool, on_yes: impl FnOnce() + 'static) {
    let d = adw::AlertDialog::new(Some(heading), Some(body));
    d.add_response("cancel", "Annuler");
    d.add_response("ok", ok);
    d.set_response_appearance(
        "ok",
        if destructive { adw::ResponseAppearance::Destructive } else { adw::ResponseAppearance::Suggested },
    );
    d.set_default_response(Some("ok"));
    d.set_close_response("cancel");
    let cb = std::cell::Cell::new(Some(on_yes));
    d.connect_response(None, move |_, r| {
        if r == "ok" {
            if let Some(f) = cb.take() {
                f();
            }
        }
    });
    d.present(Some(parent));
}

pub enum Field<'a> {
    Text(&'a str, &'a str),
    Password(&'a str),
    /// Liste déroulante : titre, choix, indice choisi ; la valeur est le texte du choix.
    Choice(&'a str, &'a [&'a str], usize),
}

#[derive(Clone)]
enum Row {
    Entry(adw::EntryRow),
    Combo(adw::ComboRow, Vec<String>),
}

impl Row {
    fn value(&self) -> String {
        match self {
            Row::Entry(r) => r.text().to_string(),
            Row::Combo(r, items) => items.get(r.selected() as usize).cloned().unwrap_or_default(),
        }
    }
}

/// Formulaire : un champ par ligne ; `on_ok` reçoit les valeurs dans l'ordre.
/// `select` : plage du premier champ à sélectionner (ex. le nom sans l'extension).
pub fn form(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    body: &str,
    fields: &[Field],
    ok: &str,
    select: Option<(i32, i32)>,
    on_ok: impl FnOnce(Vec<String>) + 'static,
) {
    let d = adw::AlertDialog::new(Some(heading), if body.is_empty() { None } else { Some(body) });
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    let mut rows: Vec<Row> = Vec::new();
    for f in fields {
        let row: adw::EntryRow = match f {
            Field::Text(title, value) => {
                let r = adw::EntryRow::new();
                r.set_title(title);
                r.set_text(value);
                r
            }
            Field::Password(title) => {
                let r = adw::PasswordEntryRow::new();
                r.set_title(title);
                r.upcast()
            }
            Field::Choice(title, items, sel) => {
                let r = adw::ComboRow::new();
                r.set_title(title);
                r.set_model(Some(&gtk::StringList::new(items)));
                r.set_selected(*sel as u32);
                list.append(&r);
                rows.push(Row::Combo(r, items.iter().map(|s| s.to_string()).collect()));
                continue;
            }
        };
        row.set_activates_default(true);
        list.append(&row);
        rows.push(Row::Entry(row));
    }
    d.set_extra_child(Some(&list));
    d.add_response("cancel", "Annuler");
    d.add_response("ok", ok);
    d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    d.set_default_response(Some("ok"));
    d.set_close_response("cancel");
    let cb = std::cell::Cell::new(Some(on_ok));
    let rows2 = rows.clone();
    d.connect_response(None, move |_, r| {
        if r == "ok" {
            if let Some(f) = cb.take() {
                f(rows2.iter().map(Row::value).collect());
            }
        }
    });
    d.present(Some(parent));
    let entries: Vec<adw::EntryRow> = rows.iter().filter_map(|r| if let Row::Entry(e) = r { Some(e.clone()) } else { None }).collect();
    if let Some(first) = entries.first().cloned() {
        // une fois affiché : focus sur le premier champ vide (ou le premier), sélection éventuelle
        let target = entries.iter().find(|r| r.text().is_empty()).cloned().unwrap_or(first.clone());
        glib::idle_add_local_once(move || {
            target.grab_focus();
            // GTK sélectionne tout le texte en prenant le focus : la sélection voulue passe après
            if let Some((a, b)) = select {
                glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || first.select_region(a, b));
            }
        });
    }
}

/// Choix de la machine pour un nouvel onglet : 1-9 puis 0, Entrée, ou une adresse saisie.
pub fn host_picker(parent: &impl IsA<gtk::Widget>, hosts: &[Host], discovering: bool) {
    let d = adw::Dialog::new();
    d.set_title("Nouvel onglet");
    d.set_content_width(560);
    let tb = adw::ToolbarView::new();
    tb.add_top_bar(&adw::HeaderBar::new());
    let col = gtk::Box::new(gtk::Orientation::Vertical, 12);
    col.set_margin_top(6);
    col.set_margin_bottom(18);
    col.set_margin_start(18);
    col.set_margin_end(18);
    let status = gtk::Label::new(Some(&if discovering {
        "Recherche des machines (mDNS, ~/.ssh/config)…".to_string()
    } else {
        format!("{} machine(s), dont {} joignable(s).", hosts.len(), hosts.iter().filter(|h| h.reachable).count())
    }));
    status.add_css_class("muted");
    status.set_xalign(0.0);
    col.append(&status);

    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    let mut targets: Vec<(Option<String>, String)> = vec![(None, "local".into())];
    targets.extend(hosts.iter().map(|h| (Some(h.target.clone()), h.label())));
    for (i, (t, label)) in targets.iter().enumerate() {
        let row = adw::ActionRow::new();
        row.set_title(&glib::markup_escape_text(label));
        let sub = match t {
            None => "cette machine".to_string(),
            Some(_) => {
                let h = &hosts[i - 1];
                let addr = h.address.clone().unwrap_or_else(|| "nom introuvable".into());
                format!("{addr}:{}  ·  {}", h.port, h.sources.iter().cloned().collect::<Vec<_>>().join(", "))
            }
        };
        row.set_subtitle(&glib::markup_escape_text(&sub));
        let key = match i {
            0..=8 => (i + 1).to_string(),
            9 => "0".into(),
            _ => String::new(),
        };
        let k = gtk::Label::new(Some(&key));
        k.add_css_class("key-hint");
        k.set_width_chars(2);
        row.add_prefix(&k);
        let on = t.is_none() || hosts[i - 1].reachable;
        let dot = gtk::Label::new(Some(if on { "●" } else { "○" }));
        dot.add_css_class(if on { "host-on" } else { "host-off" });
        row.add_prefix(&dot);
        row.set_activatable(true);
        list.append(&row);
    }
    let sw = gtk::ScrolledWindow::builder()
        .child(&list)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(420)
        .build();
    col.append(&sw);

    let addr_list = gtk::ListBox::new();
    addr_list.add_css_class("boxed-list");
    addr_list.set_selection_mode(gtk::SelectionMode::None);
    let entry = adw::EntryRow::new();
    entry.set_title("… ou une adresse : alias ssh, IP, user@hôte");
    entry.set_show_apply_button(true);
    addr_list.append(&entry);
    col.append(&addr_list);
    let hint = gtk::Label::new(Some("1-9 0 ou Entrée : ouvrir   ·   Échap : fermer"));
    hint.add_css_class("muted");
    col.append(&hint);
    tb.set_content(Some(&col));
    d.set_child(Some(&tb));

    let open = {
        let d = d.downgrade();
        let targets = targets.clone();
        move |i: usize| {
            if let Some((t, label)) = targets.get(i).cloned() {
                if let Some(d) = d.upgrade() {
                    d.close();
                }
                win().open_host(t, label, None);
            }
        }
    };
    let o = open.clone();
    list.connect_row_activated(move |_, row| o(row.index() as usize));
    let d2 = d.downgrade();
    entry.connect_apply(move |e| {
        let t = e.text().trim().to_string();
        if t.is_empty() {
            return;
        }
        if let Some(d) = d2.upgrade() {
            d.close();
        }
        win().open_host(Some(t.clone()), t, None);
    });
    let e2 = entry.clone();
    entry.connect_entry_activated(move |_| e2.emit_by_name::<()>("apply", &[]));

    let keys = gtk::EventControllerKey::new();
    let e3 = entry.clone();
    keys.connect_key_pressed(move |_, key, _, state| {
        if e3.has_focus() || e3.focus_child().is_some() || !state.is_empty() {
            return glib::Propagation::Proceed;
        }
        let digit = match key {
            gdk::Key::_1 | gdk::Key::KP_1 => 1,
            gdk::Key::_2 | gdk::Key::KP_2 => 2,
            gdk::Key::_3 | gdk::Key::KP_3 => 3,
            gdk::Key::_4 | gdk::Key::KP_4 => 4,
            gdk::Key::_5 | gdk::Key::KP_5 => 5,
            gdk::Key::_6 | gdk::Key::KP_6 => 6,
            gdk::Key::_7 | gdk::Key::KP_7 => 7,
            gdk::Key::_8 | gdk::Key::KP_8 => 8,
            gdk::Key::_9 | gdk::Key::KP_9 => 9,
            gdk::Key::_0 | gdk::Key::KP_0 => 10,
            _ => return glib::Propagation::Proceed,
        };
        open(digit - 1);
        glib::Propagation::Stop
    });
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    d.add_controller(keys);
    d.present(Some(parent));
    if let Some(r) = list.row_at_index(0) {
        r.grab_focus();
    }
}

/// Aide : les raccourcis clavier.
pub fn shortcuts(parent: &impl IsA<gtk::Widget>) {
    let rows: &[(&str, &[(&str, &str)])] = &[
        (
            "Navigation",
            &[
                ("Entrée / double-clic", "ouvrir (dossier, ou fichier avec l'application par défaut)"),
                ("Retour arrière, Alt+↑", "dossier parent"),
                ("Alt+← / Alt+→", "précédent / suivant"),
                ("Ctrl+L", "saisir un chemin (« machine:/chemin » pour une autre machine)"),
                ("taper du texte", "filtrer le dossier"),
                ("Ctrl+H", "fichiers cachés"),
                ("Ctrl+1 / Ctrl+2", "affichage en liste / en grille"),
                ("F5, Ctrl+R", "rafraîchir"),
            ],
        ),
        (
            "Fichiers",
            &[
                ("Ctrl+C / Ctrl+X / Ctrl+V", "copier / couper / coller (entre machines aussi)"),
                ("Ctrl+Maj+V", "coller en écrasant"),
                ("glisser-déposer", "copier vers un autre onglet (sur son titre) ou depuis le bureau"),
                ("F2", "renommer"),
                ("Ctrl+Maj+N / Ctrl+N", "nouveau dossier / nouveau fichier"),
                ("Suppr / Maj+Suppr", "corbeille / supprimer définitivement"),
                ("Ctrl+Maj+C", "copier le chemin"),
                ("Ctrl+Z", "annuler la dernière opération (renommage, déplacement, corbeille, création, copie)"),
                ("clic droit", "compresser, extraire, envoyer à un appareil Apple (AirDrop)…"),
            ],
        ),
        (
            "Recherche, marque-pages",
            &[
                ("Ctrl+Maj+F", "chercher dans les sous-dossiers"),
                ("Ctrl+D", "marque-page sur ce dossier"),
                ("Ctrl+B", "liste des marque-pages"),
            ],
        ),
        (
            "Fenêtre",
            &[
                ("F3", "double panneau (deux dossiers côte à côte)"),
                ("Tab", "passer à l'autre panneau"),
                ("F6 / Maj+F6", "copier / déplacer vers l'autre panneau"),
                ("Ctrl+K", "comparer, synchroniser avec l'autre panneau (ou un autre onglet)"),
                ("Ctrl+T / Ctrl+W", "nouvel onglet / fermer l'onglet"),
                ("Alt+1…9, Ctrl+Tab", "changer d'onglet"),
                ("F4", "terminal dans ce dossier (ssh à distance)"),
                ("Ctrl+G", "continuer dans l'interface terminal (mêmes onglets)"),
                ("Ctrl+Q", "quitter"),
            ],
        ),
    ];
    let d = adw::Dialog::new();
    d.set_title("Raccourcis clavier");
    d.set_content_width(640);
    let tb = adw::ToolbarView::new();
    tb.add_top_bar(&adw::HeaderBar::new());
    let page = adw::PreferencesPage::new();
    for (title, items) in rows {
        let g = adw::PreferencesGroup::new();
        g.set_title(title);
        for (k, v) in items.iter() {
            let r = adw::ActionRow::new();
            r.set_title(&glib::markup_escape_text(v));
            let l = gtk::Label::new(Some(k));
            l.add_css_class("key-hint");
            r.add_suffix(&l);
            g.add(&r);
        }
        page.add(&g);
    }
    tb.set_content(Some(&page));
    d.set_child(Some(&tb));
    d.present(Some(parent));
}
