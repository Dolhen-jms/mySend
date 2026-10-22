//! Aide de l'interface terminal (^h, ~, F1) : une entrée par ligne, touches alignées en
//! couleur, sections titrées, deux colonnes si la place le permet, recherche avec /.

use crate::input::Input;
use crate::style::*;
use crate::ui::{centered, dialog};
use crate::util::{pad, truncate};
use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

type Section = (&'static str, &'static [(&'static str, &'static str)]);

const SECTIONS: &[Section] = &[
    (
        "Déplacement",
        &[
            ("j k  ↑ ↓", "monter / descendre"),
            ("h l  ← →", "dossier parent / entrer"),
            ("gg  G", "début / fin"),
            ("^u  ^d", "demi-page"),
            ("gh gc gd g/", "aller à ~, ~/.config, ~/Downloads, /"),
            ("g Espace", "aller à un chemin saisi (Tab complète, « vps:/var/www » pour une autre machine)"),
        ],
    ),
    (
        "Ouvrir",
        &[
            ("Entrée", "dossier : entrer ; texte : l'éditeur, helix par défaut (même à distance)"),
            ("o", "application par défaut (xdg-open, open sous macOS)"),
            ("!  :shell", "shell dans ce dossier, en plein écran (ssh à distance)"),
        ],
    ),
    (
        "Sélection",
        &[
            ("Espace", "cocher"),
            ("v", "mode visuel (coche une plage)"),
            ("^a  ^r", "tout cocher / inverser"),
            ("Échap", "quitter le mode, décocher"),
        ],
    ),
    (
        "Fichiers  (les cochés, sinon le curseur)",
        &[
            ("y  x", "copier / couper"),
            ("p  P", "coller / coller en écrasant"),
            ("Y  X", "vider le presse-papiers"),
            ("d  D", "corbeille / supprimer définitivement"),
            ("a", "créer (« / » à la fin : un dossier)"),
            ("r", "renommer (plusieurs cochés : en lot, dans helix)"),
            ("C  E", "compresser / extraire l'archive"),
            ("cc cf cd", "copier le chemin / le nom / le dossier"),
            ("^z", "annuler la dernière opération"),
        ],
    ),
    (
        "Recherche, tri",
        &[
            ("f", "filtrer le dossier"),
            ("/ ?  n N", "chercher, suivant, précédent"),
            ("s  S", "chercher dans l'arborescence : par nom / dans le contenu"),
            (",n ,m ,s", "trier par nom / date / taille (majuscule : inversé)"),
            (".", "fichiers cachés"),
            ("R", "rafraîchir"),
        ],
    ),
    (
        "Onglets, machines",
        &[
            ("t", "nouvel onglet (choix de la machine)"),
            ("1…9  [ ]", "aller à l'onglet / précédent / suivant"),
            ("^Tab ^⇧Tab", "onglet suivant / précédent"),
            ("^c", "fermer l'onglet"),
            ("U", "accès refusé : sudo ou un autre utilisateur"),
            ("T", "corbeille : restaurer, supprimer, vider"),
            ("w", "transferts en cours"),
            ("A", "envoyer à un appareil Apple (AirDrop)"),
        ],
    ),
    (
        "Synchronisation  (entre deux onglets)",
        &[
            ("=1…=9", "comparer ce dossier avec celui de l'onglet N"),
            ("-1…-9", "comparer le fichier sous le curseur avec celui de l'onglet N"),
            ("> <", "cet onglet → l'autre / l'autre → cet onglet (ne supprime rien)"),
            ("u", "le plus récent gagne, dans les deux sens"),
            ("m", "dossiers : miroir de cet onglet vers l'autre (supprime le surplus)"),
            ("f", "fichiers : fusionner (conflits à trancher dans helix)"),
            ("e", "fichiers : les deux côte à côte dans helix"),
        ],
    ),
    (
        "Marque-pages",
        &[("m + lettre", "retenir ce dossier (et sa machine)"), ("'", "la liste ; ' puis la lettre pour y aller")],
    ),
    (
        "Application",
        &[
            ("^g", "passer à l'interface graphique (mêmes onglets)"),
            ("^h  ~  F1", "cette aide"),
        ],
    ),
    (
        // tout ce que comprend la ligne de commande « : » (cf. `App::command_line`)
        "Commandes  (: puis Entrée)",
        &[
            (":q  :quit", "quitter (demande confirmation si des transferts tournent)"),
            (":q!  :quit!", "quitter sans confirmation"),
            (":shell  :sh", "shell dans ce dossier, en plein écran (comme !)"),
            (":term  :!", "pareil (aussi :terminal)"),
            (":gui  :gtk", "passer à l'interface graphique (comme ^g)"),
            (":airdrop", "envoyer à un appareil Apple"),
            (":rsync", "rsync trouvé ? sa version (sinon copies par SFTP)"),
            (":help  :h", "cette aide"),
        ],
    ),
];

const KEY_W: usize = 13;

#[derive(Default)]
pub struct HelpM {
    scroll: usize,
    filter: Input,
    typing: bool,
}

/// Touche dans l'aide ; faux pour la fermer.
pub fn key(h: &mut HelpM, k: &KeyEvent, name: &str) -> bool {
    if h.typing {
        match name {
            "escape" => {
                h.filter = Input::default();
                h.typing = false;
            }
            "enter" => h.typing = false,
            _ => {
                h.filter.handle(k);
                h.scroll = 0;
            }
        }
        return true;
    }
    match name {
        "j" | "down" => h.scroll += 1,
        "k" | "up" => h.scroll = h.scroll.saturating_sub(1),
        "pagedown" | "ctrl+d" | "space" => h.scroll += 10,
        "pageup" | "ctrl+u" => h.scroll = h.scroll.saturating_sub(10),
        "g" | "home" => h.scroll = 0,
        "G" | "end" => h.scroll = usize::MAX / 2,
        "/" => h.typing = true,
        "escape" if !h.filter.text().is_empty() => h.filter = Input::default(),
        "escape" | "q" | "enter" | "?" | "f1" | "ctrl+h" | "~" => return false,
        _ => {}
    }
    true
}

/// Lignes d'une section (titre, entrées, ligne vide), filtrées ; vide si rien ne correspond.
fn section_lines(s: &Section, filter: &str, width: usize) -> Vec<Line<'static>> {
    let f = filter.to_lowercase();
    let title_hit = !f.is_empty() && s.0.to_lowercase().contains(&f);
    let rows: Vec<&(&str, &str)> =
        s.1.iter().filter(|(k, d)| f.is_empty() || title_hit || k.to_lowercase().contains(&f) || d.to_lowercase().contains(&f)).collect();
    if rows.is_empty() {
        return Vec::new();
    }
    let acc = accent();
    let mut out = vec![Line::from(vec![
        Span::styled("▍ ", fg(acc)),
        Span::styled(s.0.to_string(), ratatui::style::Style::default().fg(acc).add_modifier(Modifier::BOLD)),
    ])];
    for (k, d) in rows {
        out.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(pad(k, KEY_W), bold(YELLOW)),
            Span::raw(" "),
            Span::raw(truncate(d, width.saturating_sub(KEY_W + 3))),
        ]));
    }
    out.push(Line::from(""));
    out
}

pub fn render(f: &mut Frame, h: &mut HelpM) {
    let full = f.area();
    let w = full.width.saturating_sub(4).min(160);
    let filter = h.filter.text();
    // deux colonnes au-delà de 110 caractères
    let two = w >= 110;
    let inner_w = w.saturating_sub(4) as usize;
    let col_w = if two { (inner_w - 3) / 2 } else { inner_w };
    let blocks: Vec<Vec<Line>> = SECTIONS.iter().map(|s| section_lines(s, &filter, col_w)).filter(|b| !b.is_empty()).collect();
    let total: usize = blocks.iter().map(|b| b.len()).sum();
    let (left, right): (Vec<Line>, Vec<Line>) = if two {
        // coupure entre deux sections qui équilibre le mieux les colonnes
        // (à égalité, plus à gauche : une seule section reste à gauche)
        let prefix: Vec<usize> = std::iter::once(0).chain(blocks.iter().scan(0, |a, b| {
            *a += b.len();
            Some(*a)
        })).collect();
        let mut best = (usize::MAX, blocks.len());
        for i in (0..=blocks.len()).rev() {
            let tallest = prefix[i].max(total - prefix[i]);
            if tallest < best.0 {
                best = (tallest, i);
            }
        }
        let mut it = blocks.into_iter();
        let l: Vec<Line> = it.by_ref().take(best.1).flatten().collect();
        (l, it.flatten().collect())
    } else {
        (blocks.into_iter().flatten().collect(), Vec::new())
    };
    let body_lines = left.len().max(right.len());
    let height = (body_lines as u16 + 5).min(full.height.saturating_sub(2)).max(8);
    let inner = dialog(f, centered(full, w, height), Some("Aide · raccourcis"));
    let body_h = inner.height.saturating_sub(2) as usize;
    h.scroll = h.scroll.min(body_lines.saturating_sub(body_h));
    let take = |v: Vec<Line<'static>>| v.into_iter().skip(h.scroll).take(body_h).collect::<Vec<_>>();
    let body = Rect { height: body_h as u16, ..inner };
    if body_lines == 0 {
        f.render_widget(Paragraph::new(Line::from(Span::styled(format!("Rien ne correspond à « {filter} ».") , dim_italic()))), body);
    } else if two {
        f.render_widget(Paragraph::new(take(left)), Rect { width: col_w as u16, ..body });
        f.render_widget(Paragraph::new(take(right)), Rect { x: body.x + col_w as u16 + 3, width: col_w as u16, ..body });
    } else {
        f.render_widget(Paragraph::new(take(left)), body);
    }
    // bas : recherche en cours, ou rappel des touches
    let foot = Rect { y: inner.y + inner.height - 1, height: 1, ..inner };
    if h.typing || !filter.is_empty() {
        f.render_widget(Line::from(Span::styled("/ ", bold(accent()))), foot);
        h.filter.render(f, Rect { x: foot.x + 2, width: foot.width.saturating_sub(2), ..foot }, Default::default(), "chercher une touche ou une action", h.typing);
    } else {
        let more = if body_lines > body_h { "j k défiler · " } else { "" };
        let txt = format!("{more}/ chercher · Échap fermer · g c , m = - : la suite d'une combinaison s'affiche en bas");
        f.render_widget(Line::from(Span::styled(txt, dim())), foot);
    }
}
