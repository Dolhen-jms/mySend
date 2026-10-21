//! Couleurs : celles du thème de bureau courant (`~/.local/state/theme/current`), s'il existe ;
//! sinon celles du système (libadwaita, clair ou sombre).
//! La palette vient de `wayle.scss` (`--palette-*`), les 16 couleurs ANSI de `kitty.conf`
//! (pour la coloration syntaxique). Un `theme-set` recolore l'app à chaud.

use gtk::{gdk, gio, glib, prelude::*};
use mysend::theme::{state_dir, Palette};
use std::cell::RefCell;

trait CssExt {
    fn css(&self) -> String;
}


impl CssExt for Palette {
    fn css(&self) -> String {
        let Palette { bg, surface, elevated, fg, muted, primary, red, yellow, green, blue, .. } = self;
        // couleurs nommées (feuilles de style qui ne passent pas encore par les variables)
        let named: String = [
            ("window_bg_color", bg), ("window_fg_color", fg), ("view_bg_color", bg), ("view_fg_color", fg),
            ("headerbar_bg_color", surface), ("headerbar_fg_color", fg), ("headerbar_backdrop_color", surface),
            ("sidebar_bg_color", surface), ("sidebar_fg_color", fg), ("sidebar_backdrop_color", surface),
            ("secondary_sidebar_bg_color", surface), ("secondary_sidebar_fg_color", fg),
            ("card_bg_color", surface), ("card_fg_color", fg), ("popover_bg_color", elevated), ("popover_fg_color", fg),
            ("dialog_bg_color", surface), ("dialog_fg_color", fg), ("thumbnail_bg_color", elevated), ("thumbnail_fg_color", fg),
            ("accent_bg_color", primary), ("accent_fg_color", bg), ("accent_color", primary),
            ("destructive_bg_color", red), ("destructive_fg_color", bg), ("destructive_color", red),
            ("success_bg_color", green), ("success_color", green), ("warning_bg_color", yellow), ("warning_color", yellow),
            ("error_bg_color", red), ("error_color", red),
            ("theme_bg_color", bg), ("theme_fg_color", fg), ("theme_base_color", bg), ("theme_text_color", fg),
            ("theme_selected_bg_color", primary), ("theme_selected_fg_color", bg),
        ]
        .iter()
        .map(|(k, v)| format!("@define-color {k} {v};\n"))
        .collect();
        named + &format!(
            r#":root, window {{
  --window-bg-color: {bg}; --window-fg-color: {fg};
  --view-bg-color: {bg}; --view-fg-color: {fg};
  --headerbar-bg-color: {surface}; --headerbar-fg-color: {fg}; --headerbar-backdrop-color: {surface};
  --sidebar-bg-color: {surface}; --sidebar-fg-color: {fg}; --sidebar-backdrop-color: {surface};
  --secondary-sidebar-bg-color: {surface}; --secondary-sidebar-fg-color: {fg};
  --card-bg-color: {surface}; --card-fg-color: {fg};
  --popover-bg-color: {elevated}; --popover-fg-color: {fg};
  --dialog-bg-color: {surface}; --dialog-fg-color: {fg};
  --thumbnail-bg-color: {elevated}; --thumbnail-fg-color: {fg};
  --accent-bg-color: {primary}; --accent-fg-color: {bg}; --accent-color: {primary};
  --destructive-bg-color: {red}; --destructive-fg-color: {bg}; --destructive-color: {red};
  --success-bg-color: {green}; --success-fg-color: {bg}; --success-color: {green};
  --warning-bg-color: {yellow}; --warning-fg-color: {bg}; --warning-color: {yellow};
  --error-bg-color: {red}; --error-fg-color: {bg}; --error-color: {red};
}}
.muted {{ color: {muted}; }}
.host-on {{ color: {green}; }}
.host-off {{ color: {red}; }}
.link-name {{ font-style: italic; }}
.dir-name {{ color: {blue}; font-weight: bold; }}
.cut-item {{ opacity: 0.5; }}
gridview.file-grid {{ padding: 8px; }}
gridview.file-grid > child {{ padding: 6px; border-radius: 8px; }}
gridview.file-grid > child:selected {{ background-color: alpha({primary}, 0.28); }}
.grid-cell label {{ font-size: 0.92em; }}
.side-active {{ box-shadow: inset 0 2px 0 {primary}; }}
.pane-status {{ padding: 3px 10px; color: {muted}; font-size: 0.9em; }}
.path-bar {{ padding: 6px; }}
.path-bar entry {{ font-family: "MonoLisa", monospace; }}
.preview-text {{ font-family: "MonoLisa", monospace; padding: 8px 12px; }}
.preview-head {{ padding: 10px 12px; }}
.preview-title {{ font-weight: bold; font-size: 1.1em; }}
.drop-here {{ box-shadow: inset 0 0 0 2px {primary}; }}
.transfer-row {{ padding: 6px 10px; }}
.key-hint {{ color: {yellow}; font-family: "MonoLisa", monospace; font-weight: bold; }}
.sidebar-title {{ padding: 10px 12px 4px 12px; color: {muted}; font-size: 0.85em; font-weight: bold; }}
columnview > listview > row:selected {{ background-color: alpha({primary}, 0.28); }}
box.yank-copy {{ border-left: 4px solid {green}; padding-left: 4px; }}
box.yank-cut {{ border-left: 4px solid {red}; padding-left: 4px; }}
box.yank-cut label {{ opacity: 0.55; }}

"#
        )
    }
}

thread_local! {
    static PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    static CURRENT: RefCell<Palette> = RefCell::new(Palette::default());
}

pub fn current() -> Palette {
    CURRENT.with(|c| c.borrow().clone())
}

fn apply() {
    let p = Palette::load();
    // Pas de thème de bureau (autre machine, macOS) : on laisse libadwaita suivre le clair /
    // sombre du système, sans feuille de style ; la palette par défaut ne sert qu'à la
    // coloration syntaxique.
    let themed = mysend::theme::stamp().is_some();
    let css = if themed { p.css() } else { String::new() };
    adw::StyleManager::default().set_color_scheme(match (themed, p.is_dark()) {
        (false, _) => adw::ColorScheme::Default,
        (true, true) => adw::ColorScheme::ForceDark,
        (true, false) => adw::ColorScheme::ForceLight,
    });
    CURRENT.with(|c| *c.borrow_mut() = p);
    PROVIDER.with(|pr| {
        let mut pr = pr.borrow_mut();
        let provider = pr.get_or_insert_with(|| {
            let provider = gtk::CssProvider::new();
            provider.connect_parsing_error(|_, section, err| eprintln!("mysend-gui: CSS {}: {err}", section.to_str()));
            if let Some(d) = gdk::Display::default() {
                gtk::style_context_add_provider_for_display(&d, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
            }
            provider
        });
        provider.load_from_string(&css);
    });
}

/// Applique le thème et surveille les bascules ; `changed` est appelé après chaque recoloration.
pub fn install(changed: impl Fn() + 'static) {
    apply();
    let dir = gio::File::for_path(state_dir());
    let Ok(mon) = dir.monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) else { return };
    let pending = std::rc::Rc::new(std::cell::Cell::new(false));
    mon.connect_changed(move |_, _, _, _| {
        if pending.replace(true) {
            return;
        }
        glib::timeout_add_local_once(std::time::Duration::from_millis(300), {
            let pending = pending.clone();
            move || {
                pending.set(false);
                apply();
                CHANGED.with(|c| {
                    if let Some(f) = &*c.borrow() {
                        f();
                    }
                });
            }
        });
    });
    CHANGED.with(|c| *c.borrow_mut() = Some(Box::new(changed)));
    MONITOR.with(|m| *m.borrow_mut() = Some(mon));
}

thread_local! {
    static CHANGED: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}
