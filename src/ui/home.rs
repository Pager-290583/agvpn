//! Home screen: connection status, the big connect button and the locations list.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use crate::data::flag_emoji;
use crate::i18n::ngettext;
use crate::parsers::{Location, State};
use crate::tr;
use crate::ui::common::{escape, flag_label, format_duration_minutes, ping_label, ping_text, Rows, Ui};
use crate::util::now_secs;

pub struct HomePage {
    pub widget: gtk::Box,
    ui: Rc<Ui>,
    shield: gtk::Image,
    state_label: gtk::Label,
    detail_label: gtk::Label,
    button: gtk::Button,
    button_spinner: gtk::Spinner,
    button_label: gtk::Label,
    current_row: adw::ActionRow,
    current_flag: gtk::Label,
    current_ping: gtk::Label,
    pub search: gtk::SearchEntry,
    tab_all: gtk::ToggleButton,
    tab: RefCell<String>,
    state_box: gtk::Box,
    state_spinner: gtk::Spinner,
    state_text: gtk::Label,
    state_retry: gtk::Button,
    fastest: Rows<adw::PreferencesGroup>,
    all: Rows<adw::PreferencesGroup>,
    saved: Rows<adw::PreferencesGroup>,
}

impl HomePage {
    pub fn new(ui: Rc<Ui>) -> Rc<Self> {
        let ctl = ui.ctl.clone();
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let scroller = gtk::ScrolledWindow::builder().vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        widget.append(&scroller);
        let clamp = adw::Clamp::builder().maximum_size(560).tightening_threshold(400).build();
        scroller.set_child(Some(&clamp));
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(18)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();
        clamp.set_child(Some(&content));

        // ---------------------------------------------------------------- hero
        let hero = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(10).halign(gtk::Align::Center).build();
        hero.add_css_class("hero");
        let shield = gtk::Image::builder().icon_name("agvpn-shield-off-symbolic").pixel_size(112).build();
        shield.add_css_class("shield");
        hero.append(&shield);
        let state_label = gtk::Label::new(None);
        state_label.add_css_class("title-1");
        hero.append(&state_label);
        let detail_label = gtk::Label::builder().wrap(true).justify(gtk::Justification::Center).build();
        detail_label.add_css_class("dim-label");
        hero.append(&detail_label);

        let button = gtk::Button::builder().halign(gtk::Align::Center).margin_top(8).build();
        button.add_css_class("pill");
        button.add_css_class("connect-button");
        let btn_box = gtk::Box::builder().spacing(8).halign(gtk::Align::Center).build();
        let button_spinner = gtk::Spinner::builder().visible(false).build();
        let button_label = gtk::Label::new(None);
        btn_box.append(&button_spinner);
        btn_box.append(&button_label);
        button.set_child(Some(&btn_box));
        hero.append(&button);
        content.append(&hero);

        // ---------------------------------------------------------------- current location
        let current_list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        current_list.add_css_class("boxed-list");
        let current_row = adw::ActionRow::builder().activatable(true).build();
        let current_flag = flag_label("", "flag-large");
        current_row.add_prefix(&current_flag);
        let current_ping = gtk::Label::new(None);
        current_ping.add_css_class("ping");
        current_row.add_suffix(&current_ping);
        current_row.add_suffix(&gtk::Image::from_icon_name("go-down-symbolic"));
        current_list.append(&current_row);
        content.append(&current_list);

        // ---------------------------------------------------------------- locations toolbar
        let search = gtk::SearchEntry::builder().hexpand(true).placeholder_text(tr!("Search for a city or country")).build();
        content.append(&search);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let tabs = gtk::Box::builder().hexpand(true).homogeneous(true).build();
        tabs.add_css_class("linked");
        let tab = ctl.settings.string("locations_tab");
        let tab = if tab == "saved" { tab } else { "all".to_string() };
        let tab_all = gtk::ToggleButton::builder().label(tr!("All")).active(tab == "all").hexpand(true).build();
        let tab_saved = gtk::ToggleButton::builder().label(tr!("Saved")).active(tab == "saved").build();
        tab_saved.set_group(Some(&tab_all));
        tabs.append(&tab_all);
        tabs.append(&tab_saved);
        bar.append(&tabs);
        let refresh = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text(tr!("Update all locations")).build();
        refresh.add_css_class("flat");
        bar.append(&refresh);
        content.append(&bar);

        // ---------------------------------------------------------------- lists
        let state_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .halign(gtk::Align::Center)
            .margin_top(12)
            .visible(false)
            .build();
        let state_spinner = gtk::Spinner::builder().spinning(true).width_request(32).height_request(32).build();
        let state_text = gtk::Label::builder().wrap(true).justify(gtk::Justification::Center).use_markup(true).build();
        state_text.add_css_class("dim-label");
        let state_retry = crate::ui::common::pill_button(&tr!("Try again"), &[]);
        state_box.append(&state_spinner);
        state_box.append(&state_text);
        state_box.append(&state_retry);
        content.append(&state_box);

        let fastest = adw::PreferencesGroup::builder().title(tr!("Fastest locations")).build();
        content.append(&fastest);
        let all = adw::PreferencesGroup::builder().title(tr!("All locations")).build();
        content.append(&all);
        let saved = adw::PreferencesGroup::builder().title(tr!("Saved locations")).build();
        content.append(&saved);

        let page = Rc::new(Self {
            widget,
            ui,
            shield,
            state_label,
            detail_label,
            button,
            button_spinner,
            button_label,
            current_row,
            current_flag,
            current_ping,
            search,
            tab_all,
            tab: RefCell::new(tab),
            state_box,
            state_spinner,
            state_text,
            state_retry,
            fastest: Rows::new(fastest),
            all: Rows::new(all),
            saved: Rows::new(saved),
        });

        let weak = Rc::downgrade(&page);
        page.button.connect_clicked({
            let ctl = ctl.clone();
            move |_| {
                let state = ctl.state();
                if state.is_active() {
                    ctl.disconnect_vpn(None);
                } else if state != State::Disconnecting {
                    ctl.connect_vpn(None, false);
                }
            }
        });
        page.current_row.connect_activated({
            let search = page.search.clone();
            move |_| {
                search.grab_focus();
            }
        });
        page.search.connect_search_changed({
            let weak = weak.clone();
            move |_| {
                if let Some(page) = weak.upgrade() {
                    page.rebuild();
                }
            }
        });
        page.tab_all.connect_toggled({
            let weak = weak.clone();
            move |button| {
                if let Some(page) = weak.upgrade() {
                    let tab = if button.is_active() { "all" } else { "saved" };
                    *page.tab.borrow_mut() = tab.to_string();
                    page.ui.ctl.settings.set("locations_tab", tab);
                    page.rebuild();
                }
            }
        });
        refresh.connect_clicked({
            let ctl = ctl.clone();
            move |_| ctl.refresh_locations()
        });
        page.state_retry.connect_clicked({
            let ctl = ctl.clone();
            move |_| ctl.refresh_locations()
        });

        ctl.signals.status_changed.connect({
            let weak = weak.clone();
            move |_| {
                if let Some(page) = weak.upgrade() {
                    page.update_status();
                }
            }
        });
        ctl.signals.locations_changed.connect({
            let weak = weak.clone();
            move |_| {
                if let Some(page) = weak.upgrade() {
                    page.rebuild();
                    page.update_status();
                }
            }
        });
        glib::timeout_add_seconds_local(30, {
            let weak = weak.clone();
            move || match weak.upgrade() {
                Some(page) => {
                    page.update_status();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            }
        });
        page.update_status();
        page.rebuild();
        page
    }

    // ================================================================== status

    pub fn update_status(&self) {
        let ctl = &self.ui.ctl;
        let state = ctl.state();
        let st = ctl.status.borrow().clone();
        for class in ["status-on", "status-off", "status-busy", "status-blocked"] {
            self.shield.remove_css_class(class);
            self.button.remove_css_class(class);
        }
        self.button.remove_css_class("suggested-action");
        self.button.remove_css_class("destructive-action");

        let busy = matches!(state, State::Connecting | State::Disconnecting | State::Reconnecting);
        self.button_spinner.set_visible(busy);
        self.button_spinner.set_spinning(busy);
        let mut location = ctl.current_location();

        match state {
            State::Connected => {
                self.shield.set_icon_name(Some("agvpn-shield-symbolic"));
                self.shield.add_css_class("status-on");
                self.state_label.set_label(&tr!("Connected"));
                let mut detail = tr!("Your traffic is protected");
                let cfg = ctl.config.borrow();
                if st.mode == "SOCKS" || cfg.mode == "socks" {
                    let addr = if st.listen.is_empty() { format!("{}:{}", cfg.socks_host, cfg.socks_port) } else { st.listen.clone() };
                    detail = tr!("SOCKS5 proxy is running on {addr}", addr = addr);
                }
                if let Some(since) = ctl.connected_since.get() {
                    let minutes = ((now_secs() - since) / 60.0).floor() as i64;
                    detail.push_str(" · ");
                    detail.push_str(&format_duration_minutes(minutes));
                }
                self.detail_label.set_label(&detail);
                self.button_label.set_label(&tr!("Disconnect"));
                self.button.add_css_class("destructive-action");
            }
            State::Connecting | State::Reconnecting => {
                self.shield.set_icon_name(Some("agvpn-shield-symbolic"));
                self.shield.add_css_class("status-busy");
                self.state_label.set_label(&if state == State::Connecting { tr!("Connecting…") } else { tr!("Waiting for recovery") });
                self.detail_label.set_label(&if state == State::Reconnecting {
                    tr!("Network is lost")
                } else {
                    tr!("Establishing a secure connection")
                });
                self.button_label.set_label(&if ctl.busy.get().is_some() { tr!("Connecting") } else { tr!("Disconnect") });
            }
            State::Disconnecting => {
                self.shield.set_icon_name(Some("agvpn-shield-off-symbolic"));
                self.shield.add_css_class("status-busy");
                self.state_label.set_label(&tr!("Disconnecting…"));
                self.detail_label.set_label("");
                self.button_label.set_label(&tr!("Disconnecting"));
            }
            _ => {
                self.shield.set_icon_name(Some("agvpn-shield-off-symbolic"));
                if ctl.kill_switch_blocking() {
                    self.shield.add_css_class("status-blocked");
                    self.state_label.set_label(&tr!("Internet is blocked"));
                    self.detail_label.set_label(&tr!("VPN connection was lost. Kill Switch is blocking the Internet"));
                } else {
                    self.shield.add_css_class("status-off");
                    self.state_label.set_label(&tr!("Disconnected"));
                    let error = ctl.last_error.borrow().clone();
                    self.detail_label.set_label(&if error.is_empty() { tr!("Your traffic is not protected") } else { error });
                }
                self.button_label.set_label(&tr!("Connect"));
                self.button.add_css_class("suggested-action");
            }
        }
        self.button.set_sensitive(state != State::Disconnecting && !(state == State::Connecting && ctl.busy.get().is_some()));

        // Current / next location row
        if location.is_none() && !state.is_active() {
            let last = ctl.settings.string("last_location");
            if !last.is_empty() {
                location = ctl.locations.borrow().iter().find(|l| l.connect_arg() == last).cloned();
            }
        }
        if let Some(loc) = location {
            self.current_flag.set_label(&flag_emoji(&loc.iso));
            self.current_row.set_title(&escape(&loc.city));
            let mut subtitle = loc.country.clone();
            if loc.is_virtual {
                subtitle.push_str("  ·  ");
                subtitle.push_str(&tr!("Virtual"));
            }
            self.current_row.set_subtitle(&escape(&subtitle));
            self.current_ping.set_label(&ping_text(loc.ping));
        } else if !st.location.is_empty() {
            self.current_flag.set_label("🌐");
            self.current_row.set_title(&escape(&ctl.location_title(&st.location)));
            self.current_row.set_subtitle("");
            self.current_ping.set_label("");
        } else {
            let fastest = ctl.fastest_locations(1);
            let first = fastest.first();
            self.current_flag.set_label(&first.map(|l| flag_emoji(&l.iso)).unwrap_or_else(|| "⚡".into()));
            self.current_row.set_title(&tr!("Fastest location"));
            self.current_row.set_subtitle(&first.map(|l| escape(&format!("{}, {}", l.city, l.country))).unwrap_or_default());
            self.current_ping.set_label(&first.map(|l| ping_text(l.ping)).unwrap_or_default());
        }
    }

    // ================================================================== locations

    fn matches(loc: &Location, query: &str) -> bool {
        query.is_empty()
            || loc.city.to_lowercase().contains(query)
            || loc.country.to_lowercase().contains(query)
            || loc.iso.to_lowercase() == query
    }

    fn sorted(mut list: Vec<Location>) -> Vec<Location> {
        list.sort_by(|a, b| (&a.country, &a.city).cmp(&(&b.country, &b.city)));
        list
    }

    pub fn rebuild(self: &Rc<Self>) {
        self.fastest.clear();
        self.all.clear();
        self.saved.clear();

        let ctl = &self.ui.ctl;
        let query = self.search.text().trim().to_lowercase();
        let locations = ctl.locations.borrow().clone();
        let current_key = ctl.current_location().map(|l| l.key());

        if locations.is_empty() {
            self.fastest.group().set_visible(false);
            self.all.group().set_visible(false);
            self.saved.group().set_visible(false);
            self.state_box.set_visible(true);
            let loading = ctl.locations_loading.get();
            self.state_spinner.set_visible(loading);
            self.state_retry.set_visible(!loading);
            let error = ctl.locations_error.borrow().clone();
            self.state_text.set_label(&if loading {
                tr!("Loading locations…")
            } else {
                escape(&if error.is_empty() { tr!("Something went wrong") } else { error })
            });
            return;
        }
        self.state_box.set_visible(false);

        if *self.tab.borrow() == "saved" {
            self.fastest.group().set_visible(false);
            self.all.group().set_visible(false);
            self.saved.group().set_visible(true);
            let saved: Vec<Location> = ctl.saved_locations().into_iter().filter(|l| Self::matches(l, &query)).collect();
            let hint = tr!("To see locations here, select All and click the star next to a location");
            self.saved.group().set_description(if saved.is_empty() { Some(hint.as_str()) } else { None });
            for loc in Self::sorted(saved) {
                self.add_row(&self.saved, &loc, current_key.as_deref());
            }
            return;
        }

        self.saved.group().set_visible(false);
        self.all.group().set_visible(true);
        let fastest = if query.is_empty() { ctl.fastest_locations(3) } else { Vec::new() };
        self.fastest.group().set_visible(!fastest.is_empty());
        for loc in &fastest {
            self.add_row(&self.fastest, loc, current_key.as_deref());
        }

        let matched: Vec<Location> = locations.into_iter().filter(|l| Self::matches(l, &query)).collect();
        if query.is_empty() {
            self.all.group().set_title(&tr!("All locations"));
        } else {
            self.all.group().set_title(&ngettext("{n} location found", "{n} locations found", matched.len() as i64));
        }
        let not_found = tr!("Locations not found");
        self.all.group().set_description(if matched.is_empty() { Some(not_found.as_str()) } else { None });
        for loc in Self::sorted(matched) {
            self.add_row(&self.all, &loc, current_key.as_deref());
        }
    }

    fn add_row(self: &Rc<Self>, rows: &Rows<adw::PreferencesGroup>, loc: &Location, current_key: Option<&str>) {
        let ctl = self.ui.ctl.clone();
        let row = adw::ActionRow::builder().activatable(true).build();
        row.set_title(&escape(&loc.city));
        let mut subtitle = loc.country.clone();
        if loc.is_virtual {
            subtitle.push_str("  ·  ");
            subtitle.push_str(&tr!("Virtual"));
        }
        row.set_subtitle(&escape(&subtitle));
        row.add_prefix(&flag_label(&loc.iso, "flag"));
        if current_key == Some(loc.key().as_str()) && ctl.is_connected() {
            let check = gtk::Image::from_icon_name("object-select-symbolic");
            check.add_css_class("success");
            row.add_suffix(&check);
        }
        row.add_suffix(&ping_label(loc.ping));
        let saved = ctl.is_saved(loc);
        let star = gtk::ToggleButton::builder()
            .icon_name(if saved { "agvpn-star-symbolic" } else { "agvpn-star-outline-symbolic" })
            .active(saved)
            .valign(gtk::Align::Center)
            .tooltip_text(if saved { tr!("Remove from Saved") } else { tr!("Add to Saved") })
            .build();
        star.add_css_class("flat");
        star.add_css_class("star");
        {
            let (location, ui) = (loc.clone(), self.ui.clone());
            star.connect_toggled(move |button| {
                let saved = button.is_active();
                let (location, ctl) = (location.clone(), ui.ctl.clone());
                // Rebuilding the list destroys this button; do it after the signal returns.
                glib::idle_add_local_once(move || ctl.set_saved(&location, saved));
                if !saved {
                    ui.toast(&tr!("Location removed from Saved"));
                }
            });
        }
        row.add_suffix(&star);
        {
            let location = loc.clone();
            row.connect_activated(move |_| ctl.connect_vpn(Some(location.clone()), false));
        }
        rows.add(&row);
    }
}
