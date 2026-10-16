//! Stats screen: data usage, connection time and most used locations.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::data::flag_emoji;
use crate::stats::PERIODS;
use crate::tr;
use crate::ui::common::{confirm, escape, format_duration_minutes, pill_button, Rows, Ui};
use crate::util::human_bytes;

pub fn format_duration(secs: f64) -> String {
    format_duration_minutes((secs / 60.0).floor() as i64)
}

pub struct StatsPage {
    pub widget: gtk::Box,
    ui: Rc<Ui>,
    stack: gtk::Stack,
    period: RefCell<String>,
    down_value: gtk::Label,
    up_value: gtk::Label,
    time_value: gtk::Label,
    locations: Rows<adw::PreferencesGroup>,
    since_label: gtk::Label,
}

fn card(flow: &gtk::FlowBox, icon: &str, title: &str) -> gtk::Label {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(14)
        .margin_end(14)
        .build();
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    head.append(&gtk::Image::from_icon_name(icon));
    let caption = gtk::Label::builder().label(title).xalign(0.0).build();
    caption.add_css_class("caption");
    caption.add_css_class("dim-label");
    head.append(&caption);
    let value = gtk::Label::builder().label("—").xalign(0.0).build();
    value.add_css_class("title-2");
    content.append(&head);
    content.append(&value);
    let frame = gtk::Frame::builder().child(&content).build();
    frame.add_css_class("card");
    flow.append(&frame);
    value
}

impl StatsPage {
    pub fn new(ui: Rc<Ui>) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let stack = gtk::Stack::builder().vexpand(true).build();
        widget.append(&stack);

        // ---------------------------------------------------------------- disabled
        let disabled = adw::StatusPage::builder()
            .icon_name("agvpn-stats-symbolic")
            .title(tr!("Stats are disabled"))
            .description(tr!("View your usage time, data consumption, and most frequently used locations."))
            .build();
        let enable = pill_button(&tr!("Enable stats"), &["suggested-action"]);
        disabled.set_child(Some(&enable));
        stack.add_named(&disabled, Some("disabled"));

        // ---------------------------------------------------------------- enabled
        let pref = adw::PreferencesPage::new();
        stack.add_named(&pref, Some("enabled"));
        let top = adw::PreferencesGroup::builder().title(tr!("Stats for this computer")).build();
        pref.add(&top);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let labels: Vec<String> = PERIODS.iter().map(|(_, label)| tr!(label)).collect();
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let periods = gtk::DropDown::from_strings(&label_refs);
        periods.set_selected(1);
        periods.set_valign(gtk::Align::Center);
        header.append(&periods);
        let menu = gio::Menu::new();
        menu.append(Some(&tr!("Clear stats")), Some("stats.clear"));
        menu.append(Some(&tr!("Disable stats")), Some("stats.disable"));
        menu.append(Some(&tr!("About your stats")), Some("stats.about"));
        let more = gtk::MenuButton::builder().icon_name("view-more-symbolic").menu_model(&menu).valign(gtk::Align::Center).build();
        more.add_css_class("flat");
        header.append(&more);
        top.set_header_suffix(Some(&header));

        let cards = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(3)
            .min_children_per_line(1)
            .column_spacing(12)
            .row_spacing(12)
            .homogeneous(true)
            .build();
        let down_value = card(&cards, "go-down-symbolic", &tr!("Downloaded"));
        let up_value = card(&cards, "go-up-symbolic", &tr!("Uploaded"));
        let time_value = card(&cards, "agvpn-clock-symbolic", &tr!("Connected to VPN"));
        top.add(&cards);

        let loc_group = adw::PreferencesGroup::builder().title(tr!("Most-used locations")).build();
        pref.add(&loc_group);
        let since_label = gtk::Label::builder().margin_top(12).wrap(true).build();
        since_label.add_css_class("dim-label");
        since_label.add_css_class("caption");
        let footer = adw::PreferencesGroup::new();
        footer.add(&since_label);
        pref.add(&footer);

        let page = Rc::new(Self {
            widget,
            ui,
            stack,
            period: RefCell::new("7d".into()),
            down_value,
            up_value,
            time_value,
            locations: Rows::new(loc_group),
            since_label,
        });

        let weak = Rc::downgrade(&page);
        enable.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(page) = weak.upgrade() {
                    page.set_enabled(true);
                }
            }
        });
        periods.connect_selected_notify({
            let weak = weak.clone();
            move |dd| {
                if let Some(page) = weak.upgrade() {
                    let idx = (dd.selected() as usize).min(PERIODS.len() - 1);
                    *page.period.borrow_mut() = PERIODS[idx].0.to_string();
                    page.refresh();
                }
            }
        });

        let actions = gio::SimpleActionGroup::new();
        let handlers: [(&str, fn(&Rc<StatsPage>)); 3] =
            [("clear", Self::clear), ("disable", Self::disable), ("about", Self::about)];
        for (name, handler) in handlers {
            let action = gio::SimpleAction::new(name, None);
            let weak = weak.clone();
            action.connect_activate(move |_, _| {
                if let Some(page) = weak.upgrade() {
                    handler(&page);
                }
            });
            actions.add_action(&action);
        }
        page.widget.insert_action_group("stats", Some(&actions));

        page.ui.ctl.settings.changed.connect({
            let weak = weak.clone();
            move |key| {
                if key == "stats_enabled" || key == "*" {
                    if let Some(page) = weak.upgrade() {
                        page.refresh();
                    }
                }
            }
        });
        glib::timeout_add_seconds_local(10, {
            let weak = weak.clone();
            move || match weak.upgrade() {
                Some(page) => {
                    if page.widget.is_mapped() {
                        page.refresh();
                    }
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            }
        });
        page.widget.connect_map({
            let weak = weak.clone();
            move |_| {
                if let Some(page) = weak.upgrade() {
                    page.refresh();
                }
            }
        });
        page.refresh();
        page
    }

    fn set_enabled(&self, enabled: bool) {
        self.ui.ctl.settings.set("stats_enabled", enabled);
        self.refresh();
    }

    pub fn refresh(&self) {
        let ctl = &self.ui.ctl;
        if !ctl.settings.bool("stats_enabled") {
            self.stack.set_visible_child_name("disabled");
            return;
        }
        self.stack.set_visible_child_name("enabled");
        let summary = ctl.stats.borrow().summary(&self.period.borrow());
        self.down_value.set_label(&human_bytes(summary.rx));
        self.up_value.set_label(&human_bytes(summary.tx));
        self.time_value.set_label(&format_duration(summary.secs));

        self.locations.clear();
        if summary.locations.is_empty() {
            self.locations.add(&adw::ActionRow::builder().title(tr!("No stats yet")).build());
        }
        let total: f64 = summary.locations.iter().map(|(_, v)| v).sum::<f64>().max(1.0);
        for (name, secs) in summary.locations.iter().take(5) {
            let loc = ctl.find_location(name);
            let row = adw::ActionRow::builder()
                .title(escape(loc.as_ref().map(|l| l.city.as_str()).unwrap_or(name)))
                .subtitle(escape(loc.as_ref().map(|l| l.country.as_str()).unwrap_or("")))
                .build();
            let flag = gtk::Label::new(Some(&loc.as_ref().map(|l| flag_emoji(&l.iso)).unwrap_or_else(|| "🌐".into())));
            flag.add_css_class("flag");
            row.add_prefix(&flag);
            let bar = gtk::LevelBar::builder()
                .min_value(0.0)
                .max_value(1.0)
                .value(secs / total)
                .valign(gtk::Align::Center)
                .width_request(80)
                .build();
            row.add_suffix(&bar);
            let label = gtk::Label::new(Some(&format_duration(*secs)));
            label.add_css_class("dim-label");
            row.add_suffix(&label);
            self.locations.add(&row);
        }
        let since = if summary.since > 0.0 {
            glib::DateTime::from_unix_local(summary.since as i64)
                .and_then(|dt| dt.format("%x"))
                .map(|date| tr!("Collecting since {date}", date = date))
                .unwrap_or_default()
        } else {
            String::new()
        };
        self.since_label.set_label(&since);
    }

    fn clear(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        confirm(&self.ui.window, &tr!("Clear stats?"), &tr!("All stats will be permanently deleted"), &tr!("Clear"), true, move || {
            if let Some(page) = weak.upgrade() {
                page.ui.ctl.stats.borrow_mut().clear();
                page.refresh();
            }
        });
    }

    fn disable(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        confirm(
            &self.ui.window,
            &tr!("Disable stats?"),
            &tr!("New stats won’t be collected. You can still access your existing stats"),
            &tr!("Disable"),
            true,
            move || {
                if let Some(page) = weak.upgrade() {
                    page.set_enabled(false);
                }
            },
        );
    }

    fn about(self: &Rc<Self>) {
        let body = [
            tr!("Stats show how much VPN data you’ve used and rank your top locations"),
            tr!("Stats never include your activity or browsing history"),
            tr!("Stats are stored only on this computer and are never sent anywhere"),
        ]
        .join("\n\n");
        let dialog = adw::AlertDialog::new(Some(&tr!("About your stats")), Some(&body));
        dialog.add_response("close", &tr!("Close"));
        dialog.present(Some(&self.ui.window));
    }
}
