//! Panneau des transferts en cours (en bas de la fenêtre), avec progression et annulation.

use gtk::{glib, pango, prelude::*};
use mysend::transfers::{self, Shared};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

struct Row {
    id: u64,
    title: gtk::Label,
    detail: gtk::Label,
    bar: gtk::ProgressBar,
}

pub struct TransfersView {
    pub revealer: gtk::Revealer,
    /// Bouton de la barre de titre : nombre de transferts, affiche/masque le panneau.
    pub button: gtk::ToggleButton,
    list: gtk::Box,
    rows: RefCell<Vec<Row>>,
    shared: Shared,
}

impl TransfersView {
    pub fn new(shared: Shared) -> Rc<TransfersView> {
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        head.add_css_class("transfer-row");
        let t = gtk::Label::new(Some("Transferts"));
        t.add_css_class("heading");
        t.set_hexpand(true);
        t.set_xalign(0.0);
        let all = gtk::Button::with_label("Tout annuler");
        all.add_css_class("flat");
        head.append(&t);
        head.append(&all);
        let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
        col.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        col.append(&head);
        let sw = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(220)
            .build();
        col.append(&sw);
        let revealer = gtk::Revealer::builder().child(&col).transition_type(gtk::RevealerTransitionType::SlideUp).build();
        let button = gtk::ToggleButton::new();
        button.set_icon_name("emblem-synchronizing-symbolic");
        button.set_tooltip_text(Some("Transferts"));
        button.set_visible(false);
        let tv = Rc::new(TransfersView { revealer, button, list, rows: RefCell::default(), shared: shared.clone() });
        all.connect_clicked(move |_| transfers::cancel_all(&shared));
        let r = tv.revealer.clone();
        tv.button.connect_toggled(move |b| r.set_reveal_child(b.is_active()));
        let weak = Rc::downgrade(&tv);
        glib::timeout_add_local(Duration::from_millis(250), move || match weak.upgrade() {
            Some(tv) => {
                tv.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        tv
    }

    pub fn active(&self) -> bool {
        !self.shared.0.lock().unwrap().tasks.is_empty()
    }

    fn tick(&self) {
        let st = self.shared.0.lock().unwrap();
        let ids: Vec<u64> = st.tasks.iter().map(|t| t.id).collect();
        let had = !self.rows.borrow().is_empty();
        if ids != self.rows.borrow().iter().map(|r| r.id).collect::<Vec<_>>() {
            while let Some(c) = self.list.first_child() {
                self.list.remove(&c);
            }
            let mut rows = Vec::new();
            for t in &st.tasks {
                let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                b.add_css_class("transfer-row");
                let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
                col.set_hexpand(true);
                let title = gtk::Label::new(None);
                title.set_xalign(0.0);
                title.set_ellipsize(pango::EllipsizeMode::Middle);
                let detail = gtk::Label::new(None);
                detail.set_xalign(0.0);
                detail.add_css_class("muted");
                detail.set_ellipsize(pango::EllipsizeMode::Middle);
                let bar = gtk::ProgressBar::new();
                col.append(&title);
                col.append(&bar);
                col.append(&detail);
                let cancel = gtk::Button::from_icon_name("process-stop-symbolic");
                cancel.set_tooltip_text(Some("Annuler"));
                cancel.set_valign(gtk::Align::Center);
                cancel.add_css_class("flat");
                let (shared, id) = (self.shared.clone(), t.id);
                cancel.connect_clicked(move |_| transfers::cancel(&shared, id));
                b.append(&col);
                b.append(&cancel);
                self.list.append(&b);
                rows.push(Row { id: t.id, title, detail, bar });
            }
            *self.rows.borrow_mut() = rows;
        }
        for (t, r) in st.tasks.iter().zip(self.rows.borrow().iter()) {
            r.title.set_text(&t.title);
            match &t.running {
                Some(run) => {
                    let secs = run.started.elapsed().as_secs();
                    let mut d = format!("{}  ·  {}:{:02}", run.label, secs / 60, secs % 60);
                    if !run.extra.is_empty() {
                        d.push_str(&format!("  ·  {}", run.extra));
                    }
                    if t.cancelled {
                        d.push_str("  ·  annulation…");
                    }
                    r.detail.set_text(&d);
                    match run.pct {
                        Some(p) => r.bar.set_fraction(p as f64 / 100.0),
                        None => r.bar.pulse(),
                    }
                }
                None => {
                    r.detail.set_text("en attente");
                    r.bar.set_fraction(0.0);
                }
            }
        }
        let n = ids.len();
        drop(st);
        self.button.set_visible(n > 0);
        if n > 0 && !had {
            self.button.set_active(true);
        }
        if n == 0 {
            self.button.set_active(false);
        }
        self.button.set_tooltip_text(Some(&format!("{n} transfert(s)")));
    }
}
