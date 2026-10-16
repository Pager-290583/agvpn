//! Small shared UI helpers.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::controller::Controller;
use crate::data::flag_emoji;
use crate::tr;

/// What every page needs: the window (for dialogs and toasts) and the controller.
pub struct Ui {
    pub window: adw::ApplicationWindow,
    pub toasts: adw::ToastOverlay,
    pub ctl: Rc<Controller>,
}

impl Ui {
    pub fn gtk_window(&self) -> gtk::Window {
        self.window.clone().upcast()
    }

    pub fn toast(&self, message: &str) {
        let toast = adw::Toast::new(&glib::markup_escape_text(message));
        toast.set_timeout(4);
        self.toasts.add_toast(toast);
    }

    pub fn toast_with(&self, message: &str, button: &str, callback: impl Fn() + 'static) {
        let toast = adw::Toast::new(&glib::markup_escape_text(message));
        toast.set_timeout(4);
        toast.set_button_label(Some(button));
        toast.connect_button_clicked(move |_| callback());
        self.toasts.add_toast(toast);
    }

    pub fn show_error(&self, title: &str, detail: &str) {
        if !self.window.is_visible() {
            if let Some(app) = self.window.application() {
                crate::app::notify(&app, &self.ctl.settings, "error", title, detail);
            }
            return;
        }
        let dialog = adw::AlertDialog::new(Some(title), Some(detail));
        dialog.add_response("close", &tr!("Close"));
        dialog.present(Some(&self.window));
    }

    pub fn open_uri(&self, uri: &str) {
        crate::system::open_uri(uri, Some(&self.gtk_window()));
    }

    pub fn copy_to_clipboard(&self, text: &str) {
        WidgetExt::display(&self.window).clipboard().set_text(text);
        self.toast(&tr!("Copied to clipboard"));
    }
}

pub fn escape(text: &str) -> String {
    glib::markup_escape_text(text).to_string()
}

pub fn ping_css(ping: Option<u32>) -> &'static str {
    match ping {
        None => "ping-none",
        Some(p) if p < 100 => "ping-good",
        Some(p) if p < 200 => "ping-mid",
        Some(_) => "ping-bad",
    }
}

pub fn ping_text(ping: Option<u32>) -> String {
    match ping {
        None => tr!("Offline"),
        Some(p) if p >= 1000 => tr!("{n} s", n = format!("{:.1}", p as f64 / 1000.0)),
        Some(p) => tr!("{n} ms", n = p),
    }
}

pub fn flag_label(iso: &str, size_class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(&flag_emoji(iso)));
    label.add_css_class(size_class);
    label.set_valign(gtk::Align::Center);
    label
}

pub fn ping_label(ping: Option<u32>) -> gtk::Label {
    let label = gtk::Label::new(Some(&ping_text(ping)));
    label.add_css_class("ping");
    label.add_css_class(ping_css(ping));
    label.set_valign(gtk::Align::Center);
    label
}

pub fn nav_row(title: &str, subtitle: &str, icon: &str, on_activate: impl Fn() + 'static) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(escape(title)).subtitle(escape(subtitle)).activatable(true).build();
    if !icon.is_empty() {
        row.add_prefix(&gtk::Image::from_icon_name(icon));
    }
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    row.connect_activated(move |_| on_activate());
    row
}

pub fn link_row(title: &str, subtitle: &str, icon: &str, on_activate: impl Fn() + 'static) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(escape(title)).subtitle(escape(subtitle)).activatable(true).build();
    row.add_suffix(&gtk::Image::from_icon_name(if icon.is_empty() { "agvpn-external-symbolic" } else { icon }));
    row.connect_activated(move |_| on_activate());
    row
}

pub fn flat_icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::builder().icon_name(icon).valign(gtk::Align::Center).tooltip_text(tooltip).build();
    button.add_css_class("flat");
    button
}

pub fn pill_button(label: &str, classes: &[&str]) -> gtk::Button {
    let button = gtk::Button::builder().label(label).halign(gtk::Align::Center).build();
    button.add_css_class("pill");
    for class in classes {
        button.add_css_class(class);
    }
    button
}

pub fn confirm(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    body: &str,
    confirm_label: &str,
    destructive: bool,
    on_confirm: impl Fn() + 'static,
) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_response("cancel", &tr!("Cancel"));
    dialog.add_response("ok", confirm_label);
    dialog.set_response_appearance(
        "ok",
        if destructive { adw::ResponseAppearance::Destructive } else { adw::ResponseAppearance::Suggested },
    );
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |_, response| {
        if response == "ok" {
            on_confirm();
        }
    });
    dialog.present(Some(parent));
}

#[derive(Default)]
pub struct AskText<'a> {
    pub heading: &'a str,
    pub body: &'a str,
    pub label: &'a str,
    pub initial: &'a str,
    pub password: bool,
    pub ok_label: Option<&'a str>,
    pub validator: Option<Rc<dyn Fn(&str) -> bool>>,
}

/// Asks for one line of text. `on_done` gets `None` when cancelled.
pub fn ask_text(parent: &impl IsA<gtk::Widget>, opts: AskText, on_done: impl Fn(Option<String>) + 'static) {
    let dialog = adw::AlertDialog::new(Some(opts.heading), if opts.body.is_empty() { None } else { Some(opts.body) });
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list.add_css_class("boxed-list");
    let entry: adw::EntryRow = if opts.password {
        adw::PasswordEntryRow::builder().title(opts.label).build().upcast()
    } else {
        adw::EntryRow::builder().title(opts.label).build()
    };
    entry.set_text(opts.initial);
    list.append(&entry);
    dialog.set_extra_child(Some(&list));
    dialog.add_response("cancel", &tr!("Cancel"));
    dialog.add_response("ok", &opts.ok_label.map(String::from).unwrap_or_else(|| tr!("OK")));
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("cancel");

    let validator = opts.validator.clone();
    let update = {
        let dialog = dialog.clone();
        move |entry: &adw::EntryRow| {
            let text = entry.text().trim().to_string();
            let valid = !text.is_empty() && validator.as_ref().is_none_or(|v| v(&text));
            dialog.set_response_enabled("ok", valid);
        }
    };
    update(&entry);
    entry.connect_changed(move |e| update(e));

    let answered = Rc::new(Cell::new(false));
    let on_done = Rc::new(on_done);
    {
        let (dialog, answered, on_done) = (dialog.clone(), answered.clone(), on_done.clone());
        entry.connect_entry_activated(move |e| {
            if dialog.is_response_enabled("ok") {
                if !answered.replace(true) {
                    on_done(Some(e.text().trim().to_string()));
                }
                dialog.close();
            }
        });
    }
    {
        let entry = entry.clone();
        dialog.connect_response(None, move |_, response| {
            if answered.replace(true) {
                return;
            }
            on_done(if response == "ok" { Some(entry.text().trim().to_string()) } else { None });
        });
    }
    dialog.present(Some(parent));
    entry.grab_focus();
}

pub fn format_duration_minutes(minutes: i64) -> String {
    let (days, rem) = (minutes / 1440, minutes % 1440);
    let (hours, mins) = (rem / 60, rem % 60);
    if days > 0 {
        tr!("{d}d {h}h {m}m", d = days, h = hours, m = mins)
    } else if hours > 0 {
        tr!("{h}h {m}m", h = hours, m = mins)
    } else {
        tr!("{m}m", m = mins)
    }
}

/// A widget group whose dynamically added rows can be removed in one go.
pub struct Rows<G> {
    group: G,
    rows: std::cell::RefCell<Vec<gtk::Widget>>,
}

impl Rows<adw::PreferencesGroup> {
    pub fn new(group: adw::PreferencesGroup) -> Self {
        Self { group, rows: Default::default() }
    }

    pub fn group(&self) -> &adw::PreferencesGroup {
        &self.group
    }

    pub fn add(&self, row: &impl IsA<gtk::Widget>) {
        self.group.add(row);
        self.rows.borrow_mut().push(row.clone().upcast());
    }

    pub fn clear(&self) {
        for row in self.rows.borrow_mut().drain(..) {
            self.group.remove(&row);
        }
    }
}
