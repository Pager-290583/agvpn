//! Settings tab with its sub-pages (account, DNS, advanced, subnets, support, about).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use serde_json::{json, Value};

use crate::consts::{
    APP_NAME, APP_VERSION, SUPPORT_EMAIL, URL_ACCOUNT, URL_EULA, URL_FAQ, URL_FEEDBACK, URL_PRIVACY, URL_REPORT_BUG,
    URL_UPGRADE, URL_WEBSITE,
};
use crate::controller::error_message;
use crate::data::DNS_PRESETS;
use crate::i18n::LANGUAGES;
use crate::parsers::version_tuple;
use crate::settings::CustomDns;
use crate::system::set_autostart;
use crate::tr;
use crate::ui::common::{ask_text, confirm, escape, flat_icon_button, link_row, nav_row, pill_button, AskText, Rows, Ui};
use crate::ui::exclusions::{is_subnet, normalize_cidr};
use crate::updater::{self, Release};
use crate::util::{cache_dir, path_str, read_os_release, run_in_thread, IN_FLATPAK};

fn nav_page(title: &str, content: &impl IsA<gtk::Widget>) -> adw::NavigationPage {
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(content));
    adw::NavigationPage::new(&toolbar, title)
}

/// Suppresses widget signal handlers while the UI is updated from the model.
#[derive(Default, Clone)]
pub struct Guard(Rc<Cell<bool>>);

impl Guard {
    pub fn active(&self) -> bool {
        self.0.get()
    }

    pub fn run(&self, f: impl FnOnce()) {
        let prev = self.0.replace(true);
        f();
        self.0.set(prev);
    }
}

fn file_dialog_dismissed(err: &glib::Error) -> bool {
    err.matches(gtk::DialogError::Dismissed) || err.matches(gtk::DialogError::Cancelled)
}

pub struct SettingsTab {
    pub widget: gtk::Box,
    ui: Rc<Ui>,
    nav: adw::NavigationView,
    guard: Guard,
    account_row: adw::ActionRow,
    kill_switch: adw::SwitchRow,
    dns_row: adw::ActionRow,
    autostart: adw::SwitchRow,
    show_window: adw::SwitchRow,
    auto_connect: adw::SwitchRow,
    notifications: adw::SwitchRow,
    hide_tray: adw::SwitchRow,
    theme: adw::ComboRow,
    language: adw::ComboRow,
    pub account: RefCell<Option<Rc<AccountPage>>>,
    pub dns: RefCell<Option<Rc<DnsPage>>>,
    pub advanced: RefCell<Option<Rc<AdvancedPage>>>,
    pub subnets: RefCell<Option<Rc<SubnetsPage>>>,
    pub support: RefCell<Option<Rc<SupportPage>>>,
    pub about: RefCell<Option<Rc<AboutPage>>>,
}

impl SettingsTab {
    pub fn new(ui: Rc<Ui>) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let nav = adw::NavigationView::builder().vexpand(true).build();
        widget.append(&nav);
        let switch = |title: &str, subtitle: &str| {
            let row = adw::SwitchRow::builder().title(title).build();
            if !subtitle.is_empty() {
                row.set_subtitle(subtitle);
            }
            row
        };
        let theme_model = gtk::StringList::new(&[&tr!("System default"), &tr!("Light"), &tr!("Dark")]);
        let languages: Vec<String> =
            LANGUAGES.iter().map(|(code, name)| if *code == "system" { tr!("System default") } else { name.to_string() }).collect();
        let lang_refs: Vec<&str> = languages.iter().map(String::as_str).collect();

        let tab = Rc::new(Self {
            widget,
            ui: ui.clone(),
            nav,
            guard: Guard::default(),
            account_row: adw::ActionRow::new(),
            kill_switch: switch(&tr!("Kill Switch"), &tr!("If connection drops, Kill Switch blocks access to the Internet")),
            dns_row: adw::ActionRow::new(),
            autostart: switch(&tr!("Launch app at system login"), &tr!("AdGuard VPN can launch in the tray and connect when you log in")),
            show_window: switch(&tr!("Open Home screen automatically"), &tr!("Show the main window when the app launches at login")),
            auto_connect: switch(&tr!("Auto-connect on app launch"), ""),
            notifications: switch(&tr!("Notifications"), &tr!("Get notified about the status of the VPN connection")),
            hide_tray: switch(&tr!("Hide tray icon"), &tr!("AdGuard VPN will continue to work in the background")),
            theme: adw::ComboRow::builder().title(tr!("Theme")).model(&theme_model).build(),
            language: adw::ComboRow::builder().title(tr!("App language")).model(&gtk::StringList::new(&lang_refs)).build(),
            account: RefCell::new(None),
            dns: RefCell::new(None),
            advanced: RefCell::new(None),
            subnets: RefCell::new(None),
            support: RefCell::new(None),
            about: RefCell::new(None),
        });

        let root = tab.build_root();
        let root_page = nav_page(&tr!("Settings"), &root);
        root_page.set_tag(Some("root"));
        tab.nav.add(&root_page);

        *tab.account.borrow_mut() = Some(AccountPage::new(&tab));
        *tab.dns.borrow_mut() = Some(DnsPage::new(&tab));
        *tab.subnets.borrow_mut() = Some(SubnetsPage::new(&tab));
        *tab.advanced.borrow_mut() = Some(AdvancedPage::new(&tab));
        *tab.support.borrow_mut() = Some(SupportPage::new(&tab));
        *tab.about.borrow_mut() = Some(AboutPage::new(&tab));

        let weak = Rc::downgrade(&tab);
        let sync = move || {
            if let Some(tab) = weak.upgrade() {
                tab.sync();
            }
        };
        let s = sync.clone();
        ui.ctl.signals.license_changed.connect(move |_| s());
        let s = sync.clone();
        ui.ctl.signals.config_changed.connect(move |_| s());
        let s = sync.clone();
        ui.ctl.settings.changed.connect(move |_| s());
        tab.sync();
        tab
    }

    pub fn push(&self, page: &adw::NavigationPage) {
        self.nav.push(page);
    }

    pub fn show_advanced(&self) {
        if let Some(advanced) = self.advanced.borrow().as_ref() {
            self.push(&advanced.page);
        }
    }

    pub fn about(&self) -> Option<Rc<AboutPage>> {
        self.about.borrow().clone()
    }

    // ================================================================== root

    fn build_root(self: &Rc<Self>) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::new();
        let weak = Rc::downgrade(self);
        let push = |getter: fn(&SettingsTab) -> Option<adw::NavigationPage>| {
            let weak = weak.clone();
            move || {
                if let Some(tab) = weak.upgrade() {
                    if let Some(page) = getter(&tab) {
                        tab.push(&page);
                    }
                }
            }
        };

        let group = adw::PreferencesGroup::new();
        self.account_row.set_title(&tr!("Account"));
        self.account_row.set_activatable(true);
        self.account_row.add_prefix(&gtk::Image::from_icon_name("agvpn-account-symbolic"));
        self.account_row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let open_account = push(|t| t.account.borrow().as_ref().map(|p| p.page.clone()));
        self.account_row.connect_activated(move |_| open_account());
        group.add(&self.account_row);
        page.add(&group);

        let vpn = adw::PreferencesGroup::builder().title(tr!("VPN")).build();
        {
            let (weak, guard) = (weak.clone(), self.guard.clone());
            self.kill_switch.connect_active_notify(move |row| {
                if !guard.active() {
                    if let Some(tab) = weak.upgrade() {
                        tab.ui.ctl.set_kill_switch(row.is_active());
                    }
                }
            });
        }
        vpn.add(&self.kill_switch);
        self.dns_row.set_title(&tr!("DNS servers"));
        self.dns_row.set_activatable(true);
        self.dns_row.add_prefix(&gtk::Image::from_icon_name("agvpn-dns-symbolic"));
        self.dns_row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let open_dns = push(|t| t.dns.borrow().as_ref().map(|p| p.page.clone()));
        self.dns_row.connect_activated(move |_| open_dns());
        vpn.add(&self.dns_row);
        let open_advanced = push(|t| t.advanced.borrow().as_ref().map(|p| p.page.clone()));
        vpn.add(&nav_row(&tr!("Advanced settings"), &tr!("For expert users"), "agvpn-advanced-symbolic", open_advanced));
        page.add(&vpn);

        let app = adw::PreferencesGroup::builder().title(tr!("App settings")).build();
        {
            let (weak, guard) = (weak.clone(), self.guard.clone());
            self.autostart.connect_active_notify(move |row| {
                if guard.active() {
                    return;
                }
                if let Some(tab) = weak.upgrade() {
                    tab.ui.ctl.settings.set("autostart", row.is_active());
                    set_autostart(row.is_active());
                }
            });
        }
        app.add(&self.autostart);
        for (row, key) in [
            (&self.show_window, "show_window_on_start"),
            (&self.auto_connect, "auto_connect"),
            (&self.notifications, "notifications"),
            (&self.hide_tray, "hide_tray_icon"),
        ] {
            let (settings, guard) = (self.ui.ctl.settings.clone(), self.guard.clone());
            row.connect_active_notify(move |row| {
                if !guard.active() {
                    settings.set(key, row.is_active());
                }
            });
            app.add(row);
        }
        {
            let (ui, guard) = (self.ui.clone(), self.guard.clone());
            self.theme.connect_selected_notify(move |row| {
                if guard.active() {
                    return;
                }
                let theme = ["system", "light", "dark"].get(row.selected() as usize).copied().unwrap_or("system");
                ui.ctl.settings.set("theme", theme);
                crate::app::apply_theme(&ui.ctl.settings);
            });
        }
        app.add(&self.theme);
        {
            let (ui, guard) = (self.ui.clone(), self.guard.clone());
            self.language.connect_selected_notify(move |row| {
                if guard.active() {
                    return;
                }
                let code = LANGUAGES.get(row.selected() as usize).map(|l| l.0).unwrap_or("system");
                ui.ctl.settings.set("language", code);
                ui.toast(&tr!("To apply changes, restart the app"));
            });
        }
        app.add(&self.language);
        page.add(&app);

        let other = adw::PreferencesGroup::new();
        let open_support = push(|t| t.support.borrow().as_ref().map(|p| p.page.clone()));
        other.add(&nav_row(&tr!("Support"), &tr!("FAQ, bug reports and logs"), "agvpn-help-symbolic", open_support));
        let open_about = push(|t| t.about.borrow().as_ref().map(|p| p.page.clone()));
        other.add(&nav_row(&tr!("About"), &tr!("App version and updates"), "agvpn-info-symbolic", open_about));
        page.add(&other);

        let actions = adw::PreferencesGroup::builder().title(tr!("Actions")).build();
        let w = weak.clone();
        actions.add(&link_row(&tr!("Export settings"), &tr!("Save settings, DNS servers and exclusions to a file"), "document-save-symbolic", move || {
            if let Some(tab) = w.upgrade() {
                tab.export_settings();
            }
        }));
        let w = weak.clone();
        actions.add(&link_row(&tr!("Import settings"), &tr!("Load settings from a file"), "document-open-symbolic", move || {
            if let Some(tab) = w.upgrade() {
                tab.import_settings();
            }
        }));
        let reset = adw::ActionRow::builder().title(tr!("Reset to default")).activatable(true).build();
        reset.add_css_class("error");
        let w = weak.clone();
        reset.connect_activated(move |_| {
            let Some(tab) = w.upgrade() else { return };
            let w = Rc::downgrade(&tab);
            confirm(&tab.ui.window, &tr!("Reset to default?"), &tr!("All app settings will be reset to their default values"), &tr!("Reset"), true, move || {
                if let Some(tab) = w.upgrade() {
                    tab.reset();
                }
            });
        });
        actions.add(&reset);
        page.add(&actions);
        page
    }

    fn sync(&self) {
        let ctl = &self.ui.ctl;
        let settings = &ctl.settings;
        self.guard.run(|| {
            let lic = ctl.license.borrow();
            let plan = match lic.premium {
                None => String::new(),
                Some(true) => tr!("Unlimited"),
                Some(false) => tr!("Free"),
            };
            let subtitle: Vec<&str> = [lic.email.as_str(), plan.as_str()].into_iter().filter(|s| !s.is_empty()).collect();
            self.account_row.set_subtitle(&escape(&subtitle.join(" · ")));
            self.kill_switch.set_active(settings.bool("kill_switch"));
            self.autostart.set_active(settings.bool("autostart"));
            self.show_window.set_active(settings.bool("show_window_on_start"));
            self.auto_connect.set_active(settings.bool("auto_connect"));
            self.notifications.set_active(settings.bool("notifications"));
            self.hide_tray.set_active(settings.bool("hide_tray_icon"));
            let theme = settings.string("theme");
            self.theme.set_selected(["system", "light", "dark"].iter().position(|t| *t == theme).unwrap_or(0) as u32);
            let lang = settings.string("language");
            self.language.set_selected(LANGUAGES.iter().position(|(c, _)| *c == lang).unwrap_or(0) as u32);
            if let Some(dns) = self.dns.borrow().as_ref() {
                self.dns_row.set_subtitle(&escape(&dns.current_name()));
            }
        });
    }

    // ================================================================== actions

    fn export_settings(&self) {
        let ctl = &self.ui.ctl;
        let cfg = ctl.config.borrow().clone();
        let excl = ctl.exclusions.borrow();
        let data = json!({
            "format": "agvpn-settings-1",
            "app": ctl.settings.export(),
            "cli": {
                "mode": cfg.mode, "socks_port": cfg.socks_port, "socks_host": cfg.socks_host, "dns": cfg.dns,
                "protocol": cfg.protocol, "post_quantum": cfg.post_quantum, "routing_mode": cfg.routing_mode,
                "change_system_dns": cfg.change_system_dns, "update_channel": cfg.update_channel,
                "debug_logging": cfg.debug_logging, "bound_if": cfg.bound_if,
            },
            "exclusions": {"mode": excl.mode, "general": excl.general, "selective": excl.selective},
        });
        drop(excl);
        let dialog = gtk::FileDialog::builder().title(tr!("Export settings")).initial_name("agvpn-settings.json").build();
        let ui = self.ui.clone();
        dialog.save(Some(&self.ui.window), gio::Cancellable::NONE, move |res| {
            let Ok(file) = res else { return };
            let text = serde_json::to_string_pretty(&data).unwrap_or_default();
            match file.replace_contents(text.as_bytes(), None, false, gio::FileCreateFlags::REPLACE_DESTINATION, gio::Cancellable::NONE) {
                Ok(_) => ui.toast(&tr!("Settings exported")),
                Err(err) => ui.show_error(&tr!("Export settings"), err.message()),
            }
        });
    }

    fn import_settings(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder().title(tr!("Import settings")).build();
        let weak = Rc::downgrade(self);
        dialog.open(Some(&self.ui.window), gio::Cancellable::NONE, move |res| {
            let Some(tab) = weak.upgrade() else { return };
            let parsed = res
                .and_then(|file| file.load_contents(gio::Cancellable::NONE).map(|(data, _)| data))
                .map_err(|err| (file_dialog_dismissed(&err), err.message().to_string()))
                .and_then(|data| {
                    let value: Value = serde_json::from_slice(&data).map_err(|e| (false, e.to_string()))?;
                    if value.get("format").and_then(Value::as_str) != Some("agvpn-settings-1") {
                        return Err((false, "unknown format".to_string()));
                    }
                    Ok(value)
                });
            match parsed {
                Ok(data) => tab.apply_import(&data),
                Err((true, _)) => {}
                Err((false, message)) => tab.ui.show_error(&tr!("Failed to import settings"), &message),
            }
        });
    }

    fn apply_import(&self, data: &Value) {
        let ctl = &self.ui.ctl;
        if let Some(Value::Object(app)) = data.get("app") {
            ctl.settings.import(app);
        }
        let empty = json!({});
        let cli = data.get("cli").unwrap_or(&empty);
        let s = |key: &str| cli.get(key).and_then(Value::as_str).map(String::from);
        let onoff = |v: bool| if v { "on" } else { "off" }.to_string();
        let mut commands: Vec<(&str, String)> = Vec::new();
        if let Some(mode) = s("mode").filter(|m| m == "tun" || m == "socks") {
            commands.push(("set-mode", mode));
        }
        if let Some(port) = cli.get("socks_port").and_then(Value::as_u64).filter(|p| *p > 0) {
            commands.push(("set-socks-port", port.to_string()));
        }
        if let Some(host) = s("socks_host").filter(|h| !h.is_empty()) {
            commands.push(("set-socks-host", host));
        }
        commands.push(("set-dns", s("dns").filter(|d| !d.is_empty()).unwrap_or_else(|| "default".into())));
        if let Some(protocol) = s("protocol").filter(|p| ["auto", "http2", "quic"].contains(&p.as_str())) {
            commands.push(("set-protocol", protocol));
        }
        if let Some(pq) = cli.get("post_quantum").and_then(Value::as_bool) {
            commands.push(("set-post-quantum", onoff(pq)));
        }
        if let Some(routing) = s("routing_mode").filter(|r| ["auto", "none", "script"].contains(&r.as_str())) {
            commands.push(("set-tun-routing-mode", routing));
        }
        if let Some(sys_dns) = cli.get("change_system_dns").and_then(Value::as_bool) {
            commands.push(("set-change-system-dns", onoff(sys_dns)));
        }
        if let Some(channel) = s("update_channel").filter(|c| ["release", "beta", "nightly"].contains(&c.as_str())) {
            commands.push(("set-update-channel", channel));
        }
        if let Some(debug) = cli.get("debug_logging").and_then(Value::as_bool) {
            commands.push(("set-debug-logging", onoff(debug)));
        }
        commands.push(("set-bound-if-override", s("bound_if").unwrap_or_default()));
        for (key, value) in commands {
            ctl.set_config_quiet(key, &[&value]);
        }
        let excl = data.get("exclusions").unwrap_or(&empty);
        for mode in ["general", "selective"] {
            if let Some(items) = excl.get(mode).and_then(Value::as_array) {
                let items = items.iter().map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).collect();
                ctl.replace_exclusions(mode, items);
            }
        }
        if let Some(mode) = excl.get("mode").and_then(Value::as_str).filter(|m| *m == "general" || *m == "selective") {
            ctl.set_exclusion_mode(mode);
        }
        crate::app::apply_theme(&ctl.settings);
        self.ui.toast(&tr!("Settings imported"));
    }

    fn reset(&self) {
        let ctl = &self.ui.ctl;
        ctl.settings.reset();
        for (key, value) in [
            ("set-mode", Some("tun")),
            ("set-dns", Some("default")),
            ("set-protocol", Some("auto")),
            ("set-post-quantum", Some("on")),
            ("set-tun-routing-mode", Some("auto")),
            ("set-change-system-dns", Some("on")),
            ("set-debug-logging", Some("off")),
            ("set-bound-if-override", Some("")),
            ("set-socks-port", Some("1080")),
            ("set-socks-host", Some("127.0.0.1")),
            ("clear-socks-auth", None),
        ] {
            match value {
                Some(v) => ctl.set_config_quiet(key, &[v]),
                None => ctl.set_config_quiet(key, &[]),
            }
        }
        set_autostart(false);
        crate::app::apply_theme(&ctl.settings);
        self.ui.toast(&tr!("Settings were reset to default"));
    }
}

// =========================================================================== account

pub struct AccountPage {
    pub page: adw::NavigationPage,
    ui: Rc<Ui>,
    email: gtk::Label,
    plan_badge: gtk::Label,
    devices_value: gtk::Label,
    valid: adw::ActionRow,
    valid_value: gtk::Label,
    traffic: adw::ActionRow,
    traffic_value: gtk::Label,
    upgrade: adw::ActionRow,
}

fn value_row(title: &str) -> (adw::ActionRow, gtk::Label) {
    let row = adw::ActionRow::builder().title(title).build();
    let value = gtk::Label::new(None);
    row.add_suffix(&value);
    (row, value)
}

impl AccountPage {
    fn new(tab: &Rc<SettingsTab>) -> Rc<Self> {
        let ui = tab.ui.clone();
        let pref = adw::PreferencesPage::new();
        let head = adw::PreferencesGroup::new();
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).margin_top(6).build();
        content.append(&gtk::Image::builder().icon_name("agvpn-account-symbolic").pixel_size(64).build());
        let email = gtk::Label::builder().wrap(true).build();
        email.add_css_class("title-3");
        content.append(&email);
        let plan_badge = gtk::Label::builder().halign(gtk::Align::Center).build();
        plan_badge.add_css_class("badge");
        content.append(&plan_badge);
        head.add(&content);
        pref.add(&head);

        let info = adw::PreferencesGroup::builder().title(tr!("Subscription")).build();
        let (devices, devices_value) = value_row(&tr!("Devices"));
        let (valid, valid_value) = value_row(&tr!("Valid until"));
        let (traffic, traffic_value) = value_row(&tr!("Fast traffic left this month"));
        info.add(&devices);
        info.add(&valid);
        info.add(&traffic);
        pref.add(&info);

        let actions = adw::PreferencesGroup::new();
        let u = ui.clone();
        let upgrade = link_row(&tr!("Get subscription"), &tr!("More locations, higher speed, up to 10 devices"), "", move || u.open_uri(URL_UPGRADE));
        actions.add(&upgrade);
        let u = ui.clone();
        actions.add(&link_row(&tr!("Manage account"), &tr!("Account and subscription management"), "", move || u.open_uri(URL_ACCOUNT)));
        let refresh = adw::ActionRow::builder().title(tr!("Refresh account info")).activatable(true).build();
        refresh.add_suffix(&gtk::Image::from_icon_name("view-refresh-symbolic"));
        let ctl = ui.ctl.clone();
        refresh.connect_activated(move |_| ctl.refresh_license());
        actions.add(&refresh);
        pref.add(&actions);

        let logout_group = adw::PreferencesGroup::new();
        let logout = pill_button(&tr!("Log out"), &["destructive-action"]);
        {
            let (ui, nav) = (ui.clone(), tab.nav.clone());
            logout.connect_clicked(move |_| {
                let (ctl, nav) = (ui.ctl.clone(), nav.clone());
                confirm(
                    &ui.window,
                    &tr!("Log out?"),
                    &tr!("You’ll be logged out of your account. VPN protection will be stopped"),
                    &tr!("Log out"),
                    true,
                    move || {
                        nav.pop_to_tag("root");
                        ctl.logout();
                    },
                );
            });
        }
        logout_group.add(&logout);
        pref.add(&logout_group);

        let page = Rc::new(Self {
            page: nav_page(&tr!("Account"), &pref),
            ui: ui.clone(),
            email,
            plan_badge,
            devices_value,
            valid,
            valid_value,
            traffic,
            traffic_value,
            upgrade,
        });
        let weak = Rc::downgrade(&page);
        ui.ctl.signals.license_changed.connect(move |_| {
            if let Some(page) = weak.upgrade() {
                page.sync();
            }
        });
        let ctl = ui.ctl.clone();
        page.page.connect_showing(move |_| ctl.refresh_license());
        page.sync();
        page
    }

    fn sync(&self) {
        let lic = self.ui.ctl.license.borrow();
        self.email.set_label(&if lic.email.is_empty() { tr!("Logged in") } else { lic.email.clone() });
        self.plan_badge.set_label(&match lic.premium {
            None if lic.logged_in => tr!("Failed to get account info"),
            None => String::new(),
            Some(true) => tr!("AdGuard VPN Unlimited"),
            Some(false) => tr!("Free version"),
        });
        self.plan_badge.remove_css_class("premium");
        if lic.premium == Some(true) {
            self.plan_badge.add_css_class("premium");
        }
        self.devices_value.set_label(&match lic.devices {
            Some(n) if n > 0 => tr!("Up to {n}", n = n),
            _ => "—".into(),
        });
        let date = if lic.renew_date.is_empty() { lic.valid_until.clone() } else { lic.renew_date.clone() };
        self.valid.set_title(&if lic.renew_date.is_empty() { tr!("Valid until") } else { tr!("Renews on") });
        self.valid_value.set_label(if date.is_empty() { "—" } else { &date });
        self.valid.set_visible(lic.premium == Some(true));
        self.traffic.set_visible(lic.premium == Some(false));
        self.traffic_value.set_label(&match lic.traffic_left_gb {
            Some(gb) => tr!("{n} GB", n = format!("{gb:.2}")),
            None => "—".into(),
        });
        self.upgrade.set_visible(lic.premium != Some(true));
    }
}

// =========================================================================== DNS

pub struct DnsPage {
    pub page: adw::NavigationPage,
    ui: Rc<Ui>,
    first_radio: gtk::CheckButton,
    radios: RefCell<HashMap<String, gtk::CheckButton>>,
    custom: Rows<adw::PreferencesGroup>,
    busy: Cell<bool>,
}

impl DnsPage {
    fn new(tab: &Rc<SettingsTab>) -> Rc<Self> {
        let ui = tab.ui.clone();
        let pref = adw::PreferencesPage::new();
        let default_group = adw::PreferencesGroup::builder().description(tr!("DNS servers are used only while you are connected to VPN")).build();
        pref.add(&default_group);
        let popular = adw::PreferencesGroup::builder().title(tr!("Popular DNS servers")).build();
        pref.add(&popular);
        let custom_group = adw::PreferencesGroup::builder().title(tr!("Custom DNS servers")).build();
        let add = gtk::Button::builder().icon_name("list-add-symbolic").tooltip_text(tr!("Add custom DNS server")).build();
        add.add_css_class("flat");
        custom_group.set_header_suffix(Some(&add));
        pref.add(&custom_group);

        let page = Rc::new(Self {
            page: nav_page(&tr!("DNS servers"), &pref),
            ui: ui.clone(),
            first_radio: gtk::CheckButton::new(),
            radios: RefCell::new(HashMap::new()),
            custom: Rows::new(custom_group),
            busy: Cell::new(false),
        });
        let row = page.radio_row("", &tr!("Default"), &tr!("Uses DNS servers provided by AdGuard VPN"), Some(page.first_radio.clone()), &[]);
        default_group.add(&row);
        for preset in DNS_PRESETS {
            let row = page.radio_row(preset.address, preset.name, &tr!(preset.description), None, &[]);
            popular.add(&row);
        }

        let weak = Rc::downgrade(&page);
        add.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(page) = weak.upgrade() {
                    page.edit_dialog(None);
                }
            }
        });
        ui.ctl.signals.config_changed.connect({
            let weak = weak.clone();
            move |_| {
                if let Some(page) = weak.upgrade() {
                    page.sync();
                }
            }
        });
        ui.ctl.settings.changed.connect({
            let weak = weak.clone();
            move |key| {
                if key == "custom_dns" || key == "*" {
                    if let Some(page) = weak.upgrade() {
                        page.rebuild_custom();
                    }
                }
            }
        });
        page.rebuild_custom();
        page
    }

    pub fn current_name(&self) -> String {
        let dns = self.ui.ctl.config.borrow().dns.clone();
        if dns.is_empty() {
            return tr!("Default");
        }
        if let Some(preset) = DNS_PRESETS.iter().find(|p| p.address == dns) {
            return preset.name.to_string();
        }
        if let Some(item) = self.ui.ctl.settings.custom_dns().into_iter().find(|i| i.address == dns) {
            return if item.name.is_empty() { dns } else { item.name };
        }
        dns
    }

    fn radio_row(self: &Rc<Self>, address: &str, title: &str, subtitle: &str, check: Option<gtk::CheckButton>,
                 suffix: &[gtk::Widget]) -> adw::ActionRow {
        let check = check.unwrap_or_else(|| {
            let check = gtk::CheckButton::new();
            check.set_group(Some(&self.first_radio));
            check
        });
        let row = adw::ActionRow::builder().title(escape(title)).subtitle(escape(subtitle)).activatable_widget(&check).build();
        row.add_prefix(&check);
        for widget in suffix {
            row.add_suffix(widget);
        }
        let (weak, addr) = (Rc::downgrade(self), address.to_string());
        check.connect_toggled(move |btn| {
            if btn.is_active() {
                if let Some(page) = weak.upgrade() {
                    page.select(&addr);
                }
            }
        });
        self.radios.borrow_mut().insert(address.to_string(), check);
        row
    }

    fn select(self: &Rc<Self>, address: &str) {
        if self.busy.get() || address == self.ui.ctl.config.borrow().dns {
            return;
        }
        self.busy.set(true);
        let weak = Rc::downgrade(self);
        let value = if address.is_empty() { "default" } else { address };
        self.ui.ctl.set_config("set-dns", &[value], false, Some(Box::new(move |ok| {
            let Some(page) = weak.upgrade() else { return };
            page.busy.set(false);
            if ok {
                page.ui.toast(&tr!("You are now using “{name}” server", name = page.current_name()));
            }
            page.sync();
        })));
    }

    fn sync(&self) {
        self.busy.set(true);
        let dns = self.ui.ctl.config.borrow().dns.clone();
        if let Some(check) = self.radios.borrow().get(&dns) {
            check.set_active(true);
        }
        self.busy.set(false);
    }

    fn rebuild_custom(self: &Rc<Self>) {
        self.custom.clear();
        let presets: Vec<&str> = DNS_PRESETS.iter().map(|p| p.address).collect();
        self.radios.borrow_mut().retain(|addr, _| addr.is_empty() || presets.contains(&addr.as_str()));
        let items = self.ui.ctl.settings.custom_dns();
        if items.is_empty() {
            self.custom.add(
                &adw::ActionRow::builder()
                    .title(tr!("No custom DNS servers"))
                    .subtitle(escape(&tr!("Supported formats: IP address, tls://, https://, quic://, sdns://")))
                    .build(),
            );
        }
        for (idx, item) in items.iter().enumerate() {
            let edit = flat_icon_button("agvpn-edit-symbolic", &tr!("Edit custom DNS server"));
            let weak = Rc::downgrade(self);
            edit.connect_clicked(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.edit_dialog(Some(idx));
                }
            });
            let delete = flat_icon_button("user-trash-symbolic", &tr!("Delete"));
            let weak = Rc::downgrade(self);
            delete.connect_clicked(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.delete(idx);
                }
            });
            let title = if item.name.is_empty() { &item.address } else { &item.name };
            let row = self.radio_row(&item.address, title, &item.address, None, &[edit.upcast(), delete.upcast()]);
            self.custom.add(&row);
        }
        self.sync();
    }

    fn delete(&self, idx: usize) {
        let mut items = self.ui.ctl.settings.custom_dns();
        if idx >= items.len() {
            return;
        }
        let removed = items.remove(idx);
        self.ui.ctl.settings.set_custom_dns(&items);
        if removed.address == self.ui.ctl.config.borrow().dns {
            self.ui.ctl.set_config("set-dns", &["default"], false, None);
        }
        self.ui.toast(&tr!("DNS server deleted"));
    }

    fn edit_dialog(self: &Rc<Self>, idx: Option<usize>) {
        let items = self.ui.ctl.settings.custom_dns();
        let current = idx.and_then(|i| items.get(i).cloned()).unwrap_or(CustomDns { name: String::new(), address: String::new() });
        let dialog = adw::AlertDialog::new(
            Some(&if idx.is_some() { tr!("Edit custom DNS server") } else { tr!("Add custom DNS server") }),
            None,
        );
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        list.add_css_class("boxed-list");
        let name = adw::EntryRow::builder().title(tr!("Server name")).text(&current.name).build();
        let address = adw::EntryRow::builder().title(tr!("Server address")).text(&current.address).build();
        list.append(&name);
        list.append(&address);
        let hint_text = tr!("IP address, tls://, https://, quic:// or sdns://");
        let hint = gtk::Label::builder().label(&hint_text).wrap(true).margin_top(8).build();
        hint.add_css_class("dim-label");
        hint.add_css_class("caption");
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        outer.append(&list);
        outer.append(&hint);
        dialog.set_extra_child(Some(&outer));
        dialog.add_response("cancel", &tr!("Cancel"));
        dialog.add_response("save", &if idx.is_some() { tr!("Save") } else { tr!("Add") });
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_close_response("cancel");

        let update = {
            let (dialog, name, address, hint) = (dialog.clone(), name.clone(), address.clone(), hint.clone());
            Rc::new(move || {
                let addr = address.text().trim().to_string();
                let dup = items.iter().enumerate().any(|(i, it)| Some(i) != idx && it.address == addr);
                dialog.set_response_enabled("save", !name.text().trim().is_empty() && !addr.is_empty() && !dup);
                hint.set_label(&if dup { tr!("DNS server is already in the list") } else { hint_text.clone() });
            })
        };
        update();
        let u = update.clone();
        name.connect_changed(move |_| u());
        let u = update.clone();
        address.connect_changed(move |_| u());

        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            let Some(page) = weak.upgrade() else { return };
            if response != "save" {
                return;
            }
            let entry = CustomDns { name: name.text().trim().to_string(), address: address.text().trim().to_string() };
            let previous = page.ui.ctl.config.borrow().dns.clone();
            let page2 = page.clone();
            let addr = entry.address.clone();
            // The CLI validates the address; apply it first, keep it only if it is accepted.
            page.ui.ctl.set_config("set-dns", &[&addr], false, Some(Box::new(move |ok| {
                let ctl = &page2.ui.ctl;
                if !ok {
                    let prev = if previous.is_empty() { "default".to_string() } else { previous };
                    ctl.set_config_quiet("set-dns", &[&prev]);
                    return;
                }
                let mut items = ctl.settings.custom_dns();
                match idx {
                    Some(i) if i < items.len() => items[i] = entry,
                    _ => items.push(entry),
                }
                ctl.settings.set_custom_dns(&items);
                page2.ui.toast(&if idx.is_some() { tr!("DNS server updated") } else { tr!("DNS server added") });
            })));
        });
        dialog.present(Some(&self.ui.window));
    }
}

// =========================================================================== advanced

pub struct AdvancedPage {
    pub page: adw::NavigationPage,
    ui: Rc<Ui>,
    guard: Guard,
    mode: adw::ComboRow,
    mode_desc: gtk::Label,
    socks: adw::PreferencesGroup,
    port: adw::SpinRow,
    port_timer: RefCell<Option<glib::SourceId>>,
    host: adw::EntryRow,
    username: adw::EntryRow,
    protocol: adw::ComboRow,
    pq: adw::SwitchRow,
    ip_mode: adw::ComboRow,
    bound_if: adw::EntryRow,
    sys_dns: adw::SwitchRow,
    routing: adw::ComboRow,
    allow_lan: adw::SwitchRow,
    passwordless: adw::SwitchRow,
    helper_update: adw::ActionRow,
    logging: adw::ComboRow,
    channel: adw::ComboRow,
    auto_update: adw::SwitchRow,
    crash: adw::SwitchRow,
    telemetry: adw::SwitchRow,
}

fn combo(title: &str, items: &[String]) -> adw::ComboRow {
    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
    adw::ComboRow::builder().title(title).model(&gtk::StringList::new(&refs)).build()
}

fn switch_row(title: &str, subtitle: &str) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder().title(title).build();
    if !subtitle.is_empty() {
        row.set_subtitle(subtitle);
    }
    row
}

impl AdvancedPage {
    fn new(tab: &Rc<SettingsTab>) -> Rc<Self> {
        let ui = tab.ui.clone();
        let pref = adw::PreferencesPage::new();

        // ---------------------------------------------------------------- operating mode
        let mode_group = adw::PreferencesGroup::builder().title(tr!("Operating mode")).build();
        let mode = combo(&tr!("Mode"), &[tr!("VPN"), tr!("SOCKS5")]);
        mode_group.add(&mode);
        let mode_desc = gtk::Label::builder().wrap(true).xalign(0.0).margin_top(6).build();
        mode_desc.add_css_class("dim-label");
        mode_desc.add_css_class("caption");
        mode_group.add(&mode_desc);
        pref.add(&mode_group);

        let socks = adw::PreferencesGroup::builder().title(tr!("SOCKS5 proxy")).build();
        let port = adw::SpinRow::with_range(1.0, 65535.0, 1.0);
        port.set_title(&tr!("Port"));
        socks.add(&port);
        let host = adw::EntryRow::builder().title(tr!("Listen address")).show_apply_button(true).build();
        socks.add(&host);
        let username = adw::EntryRow::builder().title(tr!("Username")).show_apply_button(true).build();
        socks.add(&username);
        let password = adw::PasswordEntryRow::builder().title(tr!("Password")).show_apply_button(true).build();
        socks.add(&password);
        let clear = adw::ActionRow::builder().title(tr!("Clear SOCKS credentials")).activatable(true).build();
        socks.add(&clear);
        pref.add(&socks);

        // ---------------------------------------------------------------- connection
        let conn = adw::PreferencesGroup::builder().title(tr!("Connection")).build();
        let protocol = combo(&tr!("AdGuard VPN protocol"), &[tr!("Auto-select"), tr!("TrustTunnel HTTP/2"), tr!("TrustTunnel QUIC")]);
        conn.add(&protocol);
        let pq = switch_row(
            &tr!("Post-quantum cryptography"),
            &tr!("Use advanced cryptographic algorithms to prevent quantum computers from intercepting your traffic"),
        );
        conn.add(&pq);
        let ip_mode = combo(&tr!("IP version"), &[tr!("Automatic"), tr!("IPv4 only"), tr!("IPv6 only")]);
        ip_mode.set_subtitle(&tr!("Server addresses used to connect"));
        conn.add(&ip_mode);
        let bound_if = adw::EntryRow::builder().title(tr!("Outbound network interface (empty = automatic)")).show_apply_button(true).build();
        conn.add(&bound_if);
        pref.add(&conn);

        // ---------------------------------------------------------------- network
        let net = adw::PreferencesGroup::builder().title(tr!("Network")).build();
        let sys_dns = switch_row(&tr!("Change system DNS"), &tr!("Route DNS queries of the whole system through the VPN"));
        net.add(&sys_dns);
        let routing = combo(&tr!("Tunnel routing mode"), &[tr!("Automatic"), tr!("Custom script"), tr!("None")]);
        net.add(&routing);
        {
            let weak_tab = Rc::downgrade(tab);
            net.add(&nav_row(&tr!("Subnet exclusions"), &tr!("AdGuard VPN will be off for the subnets you specify"), "", move || {
                if let Some(tab) = weak_tab.upgrade() {
                    if let Some(subnets) = tab.subnets.borrow().as_ref() {
                        tab.push(&subnets.page);
                    }
                }
            }));
        }
        let allow_lan = switch_row(&tr!("Allow local network with Kill Switch"), &tr!("Printers, NAS and other LAN devices stay reachable"));
        net.add(&allow_lan);
        pref.add(&net);

        // ---------------------------------------------------------------- privileges
        let priv_group = adw::PreferencesGroup::builder().title(tr!("System access")).build();
        let passwordless = switch_row(
            &tr!("Connect without a password"),
            &tr!("Installs a helper to /usr/local/libexec/agvpn and a polkit/sudo rule for your user. Convenient, but any program running as you can then start the VPN service as root"),
        );
        priv_group.add(&passwordless);
        let helper_update = adw::ActionRow::builder()
            .title(tr!("Update the helper"))
            .subtitle(tr!("The installed helper uses an older AdGuard VPN CLI"))
            .activatable(true)
            .visible(false)
            .build();
        helper_update.add_suffix(&gtk::Image::from_icon_name("agvpn-update-symbolic"));
        priv_group.add(&helper_update);
        pref.add(&priv_group);

        // ---------------------------------------------------------------- logs, updates, privacy
        let misc = adw::PreferencesGroup::builder().title(tr!("Miscellaneous")).build();
        let logging = combo(&tr!("Logging level"), &[tr!("Default"), tr!("Extended")]);
        logging.set_subtitle(&tr!("All logs are stored locally on the device"));
        misc.add(&logging);
        let channel = combo(&tr!("Update channel"), &[tr!("Release"), tr!("Beta"), tr!("Nightly")]);
        misc.add(&channel);
        let auto_update = switch_row(&tr!("Auto-update"), &tr!("Download new versions of AdGuard VPN CLI automatically"));
        misc.add(&auto_update);
        let crash = switch_row(&tr!("Send crash reports"), "");
        misc.add(&crash);
        let telemetry = switch_row(&tr!("Send anonymized app usage data"), "");
        misc.add(&telemetry);
        pref.add(&misc);

        let page = Rc::new(Self {
            page: nav_page(&tr!("Advanced settings"), &pref),
            ui: ui.clone(),
            guard: Guard::default(),
            mode,
            mode_desc,
            socks,
            port,
            port_timer: RefCell::new(None),
            host,
            username,
            protocol,
            pq,
            ip_mode,
            bound_if,
            sys_dns,
            routing,
            allow_lan,
            passwordless,
            helper_update,
            logging,
            channel,
            auto_update,
            crash,
            telemetry,
        });
        page.connect_handlers(&password, &clear);

        let weak = Rc::downgrade(&page);
        let sync = move || {
            if let Some(page) = weak.upgrade() {
                page.sync();
            }
        };
        let s = sync.clone();
        ui.ctl.signals.config_changed.connect(move |_| s());
        let s = sync.clone();
        ui.ctl.settings.changed.connect(move |_| s());
        let weak = Rc::downgrade(&page);
        page.page.connect_showing(move |_| {
            if let Some(page) = weak.upgrade() {
                page.check_helper();
            }
        });
        page.sync();
        page
    }

    fn connect_handlers(self: &Rc<Self>, password: &adw::PasswordEntryRow, clear: &adw::ActionRow) {
        let ctl = self.ui.ctl.clone();
        let guard = self.guard.clone();
        let weak = Rc::downgrade(self);

        let combo_config = |row: &adw::ComboRow, key: &'static str, values: &'static [&'static str]| {
            let (ctl, guard) = (ctl.clone(), guard.clone());
            row.connect_selected_notify(move |row| {
                if !guard.active() {
                    if let Some(value) = values.get(row.selected() as usize) {
                        ctl.set_config(key, &[value], false, None);
                    }
                }
            });
        };
        combo_config(&self.protocol, "set-protocol", &["auto", "http2", "quic"]);
        combo_config(&self.routing, "set-tun-routing-mode", &["auto", "script", "none"]);
        combo_config(&self.logging, "set-debug-logging", &["off", "on"]);
        combo_config(&self.channel, "set-update-channel", &["release", "beta", "nightly"]);

        let switch_config = |row: &adw::SwitchRow, key: &'static str| {
            let (ctl, guard) = (ctl.clone(), guard.clone());
            row.connect_active_notify(move |row| {
                if !guard.active() {
                    ctl.set_config(key, &[if row.is_active() { "on" } else { "off" }], false, None);
                }
            });
        };
        switch_config(&self.pq, "set-post-quantum");
        switch_config(&self.sys_dns, "set-change-system-dns");
        switch_config(&self.crash, "set-crash-reporting");
        switch_config(&self.telemetry, "set-telemetry");

        let apply_text = |row: &adw::EntryRow, key: &'static str| {
            let ctl = ctl.clone();
            row.connect_apply(move |row| ctl.set_config(key, &[row.text().trim()], false, None));
        };
        apply_text(&self.host, "set-socks-host");
        apply_text(&self.username, "set-socks-username");
        apply_text(&self.bound_if, "set-bound-if-override");

        {
            let ctl = ctl.clone();
            password.connect_apply(move |row| {
                let text = row.text();
                if !text.is_empty() {
                    ctl.set_config("set-socks-password", &[&text], false, None);
                }
                row.set_text("");
            });
        }
        {
            let ctl = ctl.clone();
            clear.connect_activated(move |_| ctl.set_config("clear-socks-auth", &[], false, None));
        }
        {
            let (ui, guard) = (self.ui.clone(), guard.clone());
            self.mode.connect_selected_notify(move |row| {
                if guard.active() {
                    return;
                }
                if row.selected() == 1 {
                    let ctl = ui.ctl.clone();
                    confirm(
                        &ui.window,
                        &tr!("VPN will be turned off"),
                        &tr!("When SOCKS5 is enabled, AdGuard VPN will only be active for apps that use a SOCKS5 proxy. You will have to set up the proxy in those apps manually."),
                        &tr!("Enable SOCKS5"),
                        false,
                        move || ctl.set_config("set-mode", &["socks"], false, None),
                    );
                    let row = row.clone();
                    guard.run(|| row.set_selected(0));
                } else {
                    ui.ctl.set_config("set-mode", &["tun"], false, None);
                }
            });
        }
        {
            let (weak, guard) = (weak.clone(), guard.clone());
            self.port.connect_value_notify(move |row| {
                if guard.active() {
                    return;
                }
                let Some(page) = weak.upgrade() else { return };
                if let Some(id) = page.port_timer.borrow_mut().take() {
                    id.remove();
                }
                let (weak, row) = (weak.clone(), row.clone());
                let id = glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || {
                    if let Some(page) = weak.upgrade() {
                        page.port_timer.borrow_mut().take();
                        let port = (row.value() as i64).to_string();
                        page.ui.ctl.set_config("set-socks-port", &[&port], false, None);
                    }
                });
                *page.port_timer.borrow_mut() = Some(id);
            });
        }
        {
            let (ctl, guard) = (ctl.clone(), guard.clone());
            self.ip_mode.connect_selected_notify(move |row| {
                if guard.active() {
                    return;
                }
                let mode = ["auto", "ipv4", "ipv6"].get(row.selected() as usize).copied().unwrap_or("auto");
                ctl.settings.set("ip_mode", mode);
                if ctl.is_connected() {
                    ctl.signals.needs_reconnect.emit(());
                }
            });
        }
        {
            let (ctl, guard) = (ctl.clone(), guard.clone());
            self.allow_lan.connect_active_notify(move |row| {
                if guard.active() {
                    return;
                }
                ctl.settings.set("kill_switch_allow_lan", row.is_active());
                if ctl.kill_switch_rules_present() && ctl.is_connected() {
                    ctl.set_kill_switch(true);
                }
            });
        }
        {
            let (ctl, guard) = (ctl.clone(), guard.clone());
            self.auto_update.connect_active_notify(move |row| {
                if !guard.active() {
                    ctl.settings.set("auto_update_cli", row.is_active());
                }
            });
        }
        {
            let (weak, guard) = (weak.clone(), guard.clone());
            self.passwordless.connect_active_notify(move |row| {
                if guard.active() {
                    return;
                }
                if let Some(page) = weak.upgrade() {
                    if row.is_active() { page.install_helper() } else { page.uninstall_helper() }
                }
            });
        }
        self.helper_update.connect_activated(move |_| {
            if let Some(page) = weak.upgrade() {
                page.install_helper();
            }
        });
    }

    fn check_helper(self: &Rc<Self>) {
        let cli = self.ui.ctl.cli.clone();
        let weak = Rc::downgrade(self);
        run_in_thread(
            move || {
                let installed = cli.privileged.is_installed(true);
                (installed, installed && !cli.privileged.installed_cli_matches(&cli))
            },
            move |(installed, outdated)| {
                if let Some(page) = weak.upgrade() {
                    let row = page.passwordless.clone();
                    page.guard.run(|| row.set_active(installed));
                    page.helper_update.set_visible(outdated);
                }
            },
        );
    }

    fn install_helper(self: &Rc<Self>) {
        let cli = self.ui.ctl.cli.clone();
        let weak = Rc::downgrade(self);
        self.ui.ctl.worker.submit(
            move || cli.privileged.install(&cli),
            move |res| {
                let Some(page) = weak.upgrade() else { return };
                if !res.ok() {
                    if res.code != 126 {
                        page.ui.show_error(&tr!("Can't install the helper"), &error_message(&res));
                    }
                } else {
                    page.ui.toast(&tr!("Done. AdGuard VPN will no longer ask for a password"));
                }
                page.check_helper();
            },
        );
    }

    fn uninstall_helper(self: &Rc<Self>) {
        let cli = self.ui.ctl.cli.clone();
        let weak = Rc::downgrade(self);
        self.ui.ctl.worker.submit(
            move || cli.privileged.uninstall(&cli),
            move |res| {
                let Some(page) = weak.upgrade() else { return };
                if res.ok() {
                    page.ui.toast(&tr!("The helper was removed"));
                }
                page.check_helper();
            },
        );
    }

    fn sync(&self) {
        let ctl = &self.ui.ctl;
        let cfg = ctl.config.borrow().clone();
        let settings = &ctl.settings;
        self.guard.run(|| {
            let socks = cfg.mode == "socks";
            self.mode.set_selected(if socks { 1 } else { 0 });
            self.mode_desc.set_label(&if socks {
                tr!("AdGuard VPN runs a local proxy server that can be used by other apps to route traffic through it.")
            } else {
                tr!("All traffic is routed through AdGuard VPN automatically.")
            });
            self.socks.set_visible(socks);
            self.port.set_value(if cfg.socks_port == 0 { 1080.0 } else { f64::from(cfg.socks_port) });
            if !self.host.has_focus() {
                self.host.set_text(&cfg.socks_host);
            }
            if !self.username.has_focus() {
                self.username.set_text(&cfg.socks_username);
            }
            self.protocol.set_selected(match cfg.protocol.as_str() { "http2" => 1, "quic" => 2, _ => 0 });
            self.pq.set_active(cfg.post_quantum);
            self.ip_mode.set_selected(match settings.string("ip_mode").as_str() { "ipv4" => 1, "ipv6" => 2, _ => 0 });
            if !self.bound_if.has_focus() {
                self.bound_if.set_text(&cfg.bound_if);
            }
            self.sys_dns.set_active(cfg.change_system_dns != Some(false));
            self.routing.set_selected(match cfg.routing_mode.as_str() { "script" => 1, "none" => 2, _ => 0 });
            self.allow_lan.set_active(settings.bool("kill_switch_allow_lan"));
            self.logging.set_selected(if cfg.debug_logging { 1 } else { 0 });
            self.channel.set_selected(match cfg.update_channel.as_str() { "beta" => 1, "nightly" => 2, _ => 0 });
            self.auto_update.set_active(settings.bool("auto_update_cli"));
            self.crash.set_active(cfg.crash_reporting == Some(true));
            self.telemetry.set_active(cfg.telemetry == Some(true));
        });
    }
}

// =========================================================================== subnets

/// Subnet exclusions = CIDR entries of the General exclusions list.
pub struct SubnetsPage {
    pub page: adw::NavigationPage,
    ui: Rc<Ui>,
    rows: Rows<adw::PreferencesGroup>,
}

impl SubnetsPage {
    fn new(tab: &Rc<SettingsTab>) -> Rc<Self> {
        let ui = tab.ui.clone();
        let pref = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::builder().description(tr!("Traffic to these subnets is not routed through AdGuard VPN")).build();
        let add = gtk::Button::builder().icon_name("list-add-symbolic").tooltip_text(tr!("Add a subnet")).build();
        add.add_css_class("flat");
        group.set_header_suffix(Some(&add));
        pref.add(&group);
        let page = Rc::new(Self { page: nav_page(&tr!("Subnet exclusions"), &pref), ui: ui.clone(), rows: Rows::new(group) });
        let weak = Rc::downgrade(&page);
        add.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(page) = weak.upgrade() {
                    page.add();
                }
            }
        });
        ui.ctl.signals.exclusions_changed.connect(move |_| {
            if let Some(page) = weak.upgrade() {
                page.rebuild();
            }
        });
        page.rebuild();
        page
    }

    fn rebuild(&self) {
        self.rows.clear();
        let subnets: Vec<String> = self.ui.ctl.exclusions_for("general").into_iter().filter(|i| is_subnet(i)).collect();
        if subnets.is_empty() {
            self.rows.add(&adw::ActionRow::builder().title(tr!("No subnets")).subtitle(tr!("For example, 192.168.1.0/24")).build());
        }
        for subnet in subnets {
            let row = adw::ActionRow::builder().title(escape(&subnet)).build();
            let delete = flat_icon_button("user-trash-symbolic", &tr!("Delete"));
            let ctl = self.ui.ctl.clone();
            delete.connect_clicked(move |_| ctl.remove_exclusions(vec![subnet.clone()], Some("general"), None));
            row.add_suffix(&delete);
            self.rows.add(&row);
        }
    }

    fn add(&self) {
        let ctl = self.ui.ctl.clone();
        let validator_ctl = ctl.clone();
        let add_label = tr!("Add");
        let opts = AskText {
            heading: &tr!("Add IP subnet in CIDR notation"),
            label: &tr!("IP address"),
            ok_label: Some(&add_label),
            validator: Some(Rc::new(move |text: &str| {
                is_subnet(text) && !validator_ctl.exclusions_for("general").iter().any(|i| i == text)
            })),
            ..Default::default()
        };
        ask_text(&self.ui.window, opts, move |text| {
            if let Some(network) = text.as_deref().and_then(normalize_cidr) {
                ctl.add_exclusions(vec![network], Some("general"), Some(tr!("Subnet added")));
            }
        });
    }
}

// =========================================================================== support

pub struct SupportPage {
    pub page: adw::NavigationPage,
}

impl SupportPage {
    fn new(tab: &Rc<SettingsTab>) -> Rc<Self> {
        let ui = tab.ui.clone();
        let pref = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::new();
        let u = ui.clone();
        group.add(&link_row(&tr!("FAQ"), &tr!("Frequently asked questions"), "", move || u.open_uri(URL_FAQ)));
        let u = ui.clone();
        group.add(&link_row(&tr!("Report a bug"), &tr!("Send us a report about technical issues"), "", move || report_bug(&u)));
        let u = ui.clone();
        group.add(&link_row(&tr!("Leave feedback"), &tr!("Submit feature requests and suggestions"), "", move || u.open_uri(URL_FEEDBACK)));
        let u = ui.clone();
        group.add(&link_row(&tr!("Contact support"), SUPPORT_EMAIL, "agvpn-mail-symbolic", move || {
            u.open_uri(&format!("mailto:{SUPPORT_EMAIL}"))
        }));
        pref.add(&group);
        let logs = adw::PreferencesGroup::new();
        let u = ui.clone();
        logs.add(&link_row(
            &tr!("Export logs and system info…"),
            &tr!("Save logs as a ZIP file for troubleshooting or sharing"),
            "document-save-symbolic",
            move || export_logs(&u),
        ));
        pref.add(&logs);
        Rc::new(Self { page: nav_page(&tr!("Support"), &pref) })
    }
}

fn report_bug(ui: &Rc<Ui>) {
    let dialog = adw::AlertDialog::new(
        Some(&tr!("Report a bug")),
        Some(&tr!("Export logs and system info first, then attach the ZIP file to your report. Logs never include your browsing history.")),
    );
    dialog.add_response("cancel", &tr!("Cancel"));
    dialog.add_response("logs", &tr!("Export logs"));
    dialog.add_response("open", &tr!("Open bug tracker"));
    dialog.set_response_appearance("open", adw::ResponseAppearance::Suggested);
    let ui2 = ui.clone();
    dialog.connect_response(None, move |_, response| match response {
        "logs" => export_logs(&ui2),
        "open" => ui2.open_uri(URL_REPORT_BUG),
        _ => {}
    });
    dialog.present(Some(&ui.window));
}

fn export_logs(ui: &Rc<Ui>) {
    let dialog = gtk::FileDialog::builder().title(tr!("Export logs")).initial_name("agvpn-logs.zip").build();
    let ui2 = ui.clone();
    dialog.save(Some(&ui.window), gio::Cancellable::NONE, move |res| {
        let Ok(file) = res else { return };
        ui2.toast(&tr!("Exporting logs…"));
        let cli = ui2.ctl.cli.clone();
        let ui3 = ui2.clone();
        run_in_thread(
            move || build_logs_zip(&cli),
            move |result| {
                let written = result.and_then(|data| {
                    file.replace_contents(&data, None, false, gio::FileCreateFlags::REPLACE_DESTINATION, gio::Cancellable::NONE)
                        .map(|_| ())
                        .map_err(|e| e.message().to_string())
                });
                match written {
                    Ok(()) => ui3.toast(&tr!("Export of logs and system information completed")),
                    Err(err) => ui3.show_error(&tr!("Export of logs and system information failed"), &err),
                }
            },
        );
    });
}

fn uname() -> String {
    let mut info: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut info) } != 0 {
        return String::new();
    }
    let field = |raw: &[libc::c_char]| {
        let bytes: Vec<u8> = raw.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    format!("{} {} {} {}", field(&info.sysname), field(&info.release), field(&info.version), field(&info.machine))
}

fn build_logs_zip(cli: &crate::cli::Cli) -> Result<Vec<u8>, String> {
    use std::io::Write;
    use std::time::Duration;
    use zip::write::SimpleFileOptions;

    let tmp = cache_dir().join("cli-logs.zip");
    let res = cli.export_logs(&path_str(&tmp));
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buffer);
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut add = |name: &str, data: &[u8]| -> Result<(), String> {
            zip.start_file(name, opts).map_err(|e| e.to_string())?;
            zip.write_all(data).map_err(|e| e.to_string())
        };
        match std::fs::read(&tmp) {
            Ok(data) if res.ok() => add("adguardvpn-cli-logs.zip", &data)?,
            _ => add("adguardvpn-cli-export-error.txt", res.out.as_bytes())?,
        }
        if let Ok(data) = std::fs::read(cache_dir().join("agvpn.log")) {
            add("agvpn-app.log", &data)?;
        }
        let run = |args: &[&str]| cli.run(args, Duration::from_secs(60)).text();
        let os = read_os_release();
        let mut info = vec![
            format!("{APP_NAME} {APP_VERSION} (Rust)"),
            format!("CLI: {} v{}", cli.binary().unwrap_or_default(), cli.version()),
            format!("Flatpak: {}", *IN_FLATPAK),
            format!("Platform: {} {}", uname(), os.get("PRETTY_NAME").cloned().unwrap_or_default()),
            String::new(),
            "== status ==".into(),
            run(&["status"]),
            "== config ==".into(),
            run(&["config", "show"]),
            "== exclusion mode ==".into(),
            run(&["site-exclusions", "mode"]),
        ];
        if let Ok(text) = std::fs::read_to_string("/etc/os-release") {
            info.extend([String::new(), "== os-release ==".into(), text]);
        }
        if *IN_FLATPAK {
            if let Ok(text) = std::fs::read_to_string("/.flatpak-info") {
                info.extend([String::new(), "== flatpak-info ==".into(), text]);
            }
        }
        add("system-info.txt", info.join("\n").as_bytes())?;
        zip.finish().map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(&tmp);
    Ok(buffer.into_inner())
}

// =========================================================================== about

pub struct AboutPage {
    pub page: adw::NavigationPage,
    ui: Rc<Ui>,
    pub release: RefCell<Option<Release>>,
    cli_version: adw::ActionRow,
    update_row: adw::ActionRow,
    update_spinner: gtk::Spinner,
    pub update_button: gtk::Button,
}

impl AboutPage {
    fn new(tab: &Rc<SettingsTab>) -> Rc<Self> {
        let ui = tab.ui.clone();
        let pref = adw::PreferencesPage::new();
        let head = adw::PreferencesGroup::new();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content.append(&gtk::Image::builder().icon_name("io.github.agvpn.AGVPN").pixel_size(96).build());
        let title = gtk::Label::new(Some(APP_NAME));
        title.add_css_class("title-1");
        content.append(&title);
        let subtitle = gtk::Label::builder()
            .label(tr!("Unofficial desktop client for AdGuard VPN"))
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        subtitle.add_css_class("dim-label");
        content.append(&subtitle);
        head.add(&content);
        pref.add(&head);

        let versions = adw::PreferencesGroup::builder().title(tr!("Versions")).build();
        versions.add(&adw::ActionRow::builder().title(tr!("App version")).subtitle(APP_VERSION).build());
        let cli_version = adw::ActionRow::builder().title(tr!("AdGuard VPN CLI version")).build();
        let copy = flat_icon_button("edit-copy-symbolic", &tr!("Copy app version"));
        cli_version.add_suffix(&copy);
        versions.add(&cli_version);
        let update_row = adw::ActionRow::builder().title(tr!("Check for updates")).activatable(true).build();
        let update_spinner = gtk::Spinner::new();
        update_row.add_suffix(&update_spinner);
        let update_button = gtk::Button::builder().label(tr!("Update now")).valign(gtk::Align::Center).visible(false).build();
        update_button.add_css_class("suggested-action");
        update_row.add_suffix(&update_button);
        versions.add(&update_row);
        pref.add(&versions);

        let links = adw::PreferencesGroup::new();
        let u = ui.clone();
        links.add(&link_row(&tr!("Official website"), URL_WEBSITE, "", move || u.open_uri(URL_WEBSITE)));
        let u = ui.clone();
        links.add(&link_row(&tr!("End-user license agreement"), "", "", move || u.open_uri(URL_EULA)));
        let u = ui.clone();
        links.add(&link_row(&tr!("Privacy policy"), "", "", move || u.open_uri(URL_PRIVACY)));
        pref.add(&links);

        let legal = adw::PreferencesGroup::new();
        let note = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .label(tr!("This app is not affiliated with AdGuard. It is a graphical front-end for the official AdGuard VPN CLI, which is downloaded from AdGuard's GitHub releases and is subject to AdGuard's license agreement."))
            .build();
        note.add_css_class("dim-label");
        note.add_css_class("caption");
        legal.add(&note);
        pref.add(&legal);

        let page = Rc::new(Self {
            page: nav_page(&tr!("About"), &pref),
            ui: ui.clone(),
            release: RefCell::new(None),
            cli_version,
            update_row,
            update_spinner,
            update_button,
        });
        {
            let ui = ui.clone();
            copy.connect_clicked(move |_| {
                ui.copy_to_clipboard(&format!("{APP_NAME} {APP_VERSION}, AdGuard VPN CLI {}", ui.ctl.cli.version()));
            });
        }
        let weak = Rc::downgrade(&page);
        page.update_button.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(page) = weak.upgrade() {
                    page.install_update();
                }
            }
        });
        page.update_row.connect_activated({
            let weak = weak.clone();
            move |_| {
                if let Some(page) = weak.upgrade() {
                    page.check_updates();
                }
            }
        });
        ui.ctl.signals.cli_changed.connect(move |_| {
            if let Some(page) = weak.upgrade() {
                page.sync();
            }
        });
        page.sync();
        page
    }

    fn sync(&self) {
        let version = self.ui.ctl.cli.version();
        self.cli_version.set_subtitle(&if version.is_empty() { tr!("Not installed") } else { version });
    }

    pub fn offer_release(&self, rel: Release) {
        *self.release.borrow_mut() = Some(rel);
        self.update_button.set_visible(true);
    }

    fn check_updates(self: &Rc<Self>) {
        self.update_spinner.set_spinning(true);
        self.update_row.set_subtitle(&tr!("Checking for updates..."));
        let channel = self.ui.ctl.config.borrow().update_channel.clone();
        let channel = if channel.is_empty() { "release".to_string() } else { channel };
        let weak = Rc::downgrade(self);
        run_in_thread(
            move || updater::latest_release(&channel),
            move |result| {
                let Some(page) = weak.upgrade() else { return };
                page.update_spinner.set_spinning(false);
                let rel = match result {
                    Err(_) => {
                        page.update_row.set_subtitle(&tr!("Failed to check for updates"));
                        return;
                    }
                    Ok(None) => {
                        page.update_row.set_subtitle(&tr!("You are using the latest version"));
                        return;
                    }
                    Ok(Some(rel)) => rel,
                };
                let current = page.ui.ctl.cli.version();
                if !current.is_empty() && version_tuple(&rel.version) <= version_tuple(&current) {
                    page.update_row.set_subtitle(&tr!("You are using the latest version"));
                    page.update_button.set_visible(false);
                    return;
                }
                let message = tr!("A newer version {version} is available", version = rel.version);
                page.update_row.set_subtitle(&message);
                page.offer_release(rel);
                page.ui.toast(&message);
            },
        );
    }

    fn install_update(self: &Rc<Self>) {
        let Some(rel) = self.release.borrow().clone() else { return };
        self.update_button.set_sensitive(false);
        self.update_spinner.set_spinning(true);
        self.update_row.set_subtitle(&tr!("Downloading {version}…", version = rel.version));
        let weak = Rc::downgrade(self);
        run_in_thread(
            move || updater::install_release(&rel),
            move |result| {
                let Some(page) = weak.upgrade() else { return };
                page.update_spinner.set_spinning(false);
                page.update_button.set_sensitive(true);
                if let Err(err) = result {
                    page.update_row.set_subtitle(&err);
                    page.ui.show_error(&tr!("Update failed"), &err);
                    return;
                }
                page.update_button.set_visible(false);
                let weak = Rc::downgrade(&page);
                page.ui.ctl.reload_cli(Some(Box::new(move || {
                    if let Some(page) = weak.upgrade() {
                        page.update_row.set_subtitle(&tr!("Updated to version {version}", version = page.ui.ctl.cli.version()));
                    }
                })));
                let cli = page.ui.ctl.cli.clone();
                let ui = page.ui.clone();
                run_in_thread(move || cli.privileged.is_installed(false), move |installed| {
                    if installed {
                        ui.toast(&tr!("Update the passwordless helper in Advanced settings"));
                    }
                });
            },
        );
    }
}
