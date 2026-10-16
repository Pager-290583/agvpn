//! Exclusions: General / Selective mode, websites, services, import & export.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::rc::Rc;
use std::sync::LazyLock;

use adw::prelude::*;
use gtk::{gio, glib};
use regex::Regex;

use crate::data::{service_by_id, service_patterns, SERVICES};
use crate::i18n::ngettext;
use crate::tr;
use crate::ui::common::{confirm, escape, flat_icon_button, Rows, Ui};

static DOMAIN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\*\.)?([a-z0-9_]([a-z0-9_-]{0,61}[a-z0-9_])?\.)+[a-z0-9-]{2,63}$").unwrap());

/// Parses "addr/prefix" (like Python's `ip_network(strict=False)`).
pub fn parse_cidr(value: &str) -> Option<(IpAddr, u8)> {
    let (addr, prefix) = value.split_once('/')?;
    let addr: IpAddr = addr.parse().ok()?;
    if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let prefix: u8 = prefix.parse().ok()?;
    let max = if addr.is_ipv4() { 32 } else { 128 };
    (prefix <= max).then_some((addr, prefix))
}

/// The network address of a CIDR, e.g. "192.168.1.7/24" -> "192.168.1.0/24".
pub fn normalize_cidr(value: &str) -> Option<String> {
    let (addr, prefix) = parse_cidr(value)?;
    let network = match addr {
        IpAddr::V4(v4) => {
            let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
            IpAddr::from(std::net::Ipv4Addr::from(u32::from(v4) & mask))
        }
        IpAddr::V6(v6) => {
            let mask = if prefix == 0 { 0 } else { u128::MAX << (128 - prefix) };
            IpAddr::from(std::net::Ipv6Addr::from(u128::from(v6) & mask))
        }
    };
    Some(format!("{network}/{prefix}"))
}

pub fn is_subnet(item: &str) -> bool {
    parse_cidr(item).is_some()
}

/// Turns user input (URL, domain, IP, CIDR) into a CLI exclusion pattern, or None.
pub fn normalize_entry(text: &str) -> Option<String> {
    static SCHEME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9+.-]*://").unwrap());
    static V4_PORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{1,3}\.){3}\d{1,3}:\d{1,5}$").unwrap());
    static V6_PORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[[0-9a-f:]+\]:\d{1,5}$").unwrap());

    let value = text.trim().to_lowercase();
    if value.is_empty() {
        return None;
    }
    let mut value = SCHEME.replace(&value, "").into_owned();
    if let Some((head, _)) = value.split_once('/') {
        if parse_cidr(&value).is_some() {
            return Some(value);
        }
        value = head.to_string();
    }
    let value = value.split('?').next().unwrap_or("").split('#').next().unwrap_or("").trim_end_matches('.').to_string();
    if let Ok(ip) = value.trim_matches(['[', ']']).parse::<IpAddr>() {
        return Some(ip.to_string());
    }
    // IPv4:port / [IPv6]:port
    if V4_PORT.is_match(&value) || V6_PORT.is_match(&value) {
        return Some(value);
    }
    let value = value.split(':').next().unwrap_or("").to_string();
    let (wildcard, host) = match value.strip_prefix("*.") {
        Some(rest) => (true, rest.to_string()),
        None => (false, value),
    };
    if host.is_empty() || host.split('.').any(str::is_empty) {
        return None;
    }
    let ascii = idna::domain_to_ascii(&host).ok()?;
    let value = if wildcard { format!("*.{ascii}") } else { ascii };
    DOMAIN_RE.is_match(&value).then_some(value)
}

pub fn is_ip_like(item: &str) -> bool {
    static IP_CHARS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\d.:\[\]/a-f]+$").unwrap());
    IP_CHARS.is_match(item)
        && item.chars().any(|c| c.is_ascii_digit())
        && (item.contains('/') || item.contains(':') || item.matches('.').count() == 3)
}

/// Returns (base, has_domain, has_subdomains) preserving order.
pub fn group_domains(items: &[String]) -> Vec<(String, bool, bool)> {
    let mut order: Vec<String> = Vec::new();
    let mut info: HashMap<String, (bool, bool)> = HashMap::new();
    for item in items {
        let (base, sub) = match item.strip_prefix("*.") {
            Some(base) => (base.to_string(), true),
            None => (item.clone(), false),
        };
        let entry = info.entry(base.clone()).or_insert_with(|| {
            order.push(base);
            (false, false)
        });
        if sub {
            entry.1 = true;
        } else {
            entry.0 = true;
        }
    }
    order.into_iter().map(|b| (b.clone(), info[&b].0, info[&b].1)).collect()
}

pub struct ExclusionsPage {
    pub widget: gtk::Box,
    ui: Rc<Ui>,
    general_check: gtk::CheckButton,
    selective_check: gtk::CheckButton,
    mode_handler: RefCell<Option<glib::SignalHandlerId>>,
    sites: Rows<adw::PreferencesGroup>,
    search: adw::EntryRow,
}

impl ExclusionsPage {
    pub fn new(ui: Rc<Ui>) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let pref = adw::PreferencesPage::builder().vexpand(true).build();
        widget.append(&pref);

        // ---------------------------------------------------------------- mode
        let mode_group = adw::PreferencesGroup::builder().title(tr!("Mode")).description(tr!("Manage your VPN per website")).build();
        pref.add(&mode_group);
        let general_check = gtk::CheckButton::new();
        let selective_check = gtk::CheckButton::new();
        selective_check.set_group(Some(&general_check));
        let general_row = adw::ActionRow::builder()
            .title(tr!("General"))
            .subtitle(tr!("VPN is active everywhere except for excluded websites"))
            .activatable_widget(&general_check)
            .build();
        general_row.add_prefix(&general_check);
        let selective_row = adw::ActionRow::builder()
            .title(tr!("Selective"))
            .subtitle(tr!("VPN is active only on selected websites"))
            .activatable_widget(&selective_check)
            .build();
        selective_row.add_prefix(&selective_check);
        mode_group.add(&general_row);
        mode_group.add(&selective_row);

        // ---------------------------------------------------------------- websites
        let sites_group = adw::PreferencesGroup::new();
        pref.add(&sites_group);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let add_menu = gio::Menu::new();
        add_menu.append(Some(&tr!("Add website or IP…")), Some("excl.add"));
        add_menu.append(Some(&tr!("Add from popular services…")), Some("excl.services"));
        let add_btn = gtk::MenuButton::builder().icon_name("list-add-symbolic").menu_model(&add_menu).tooltip_text(tr!("Add")).build();
        add_btn.add_css_class("flat");
        let more_menu = gio::Menu::new();
        more_menu.append(Some(&tr!("Import list…")), Some("excl.import"));
        more_menu.append(Some(&tr!("Export list…")), Some("excl.export"));
        more_menu.append(Some(&tr!("Clear list")), Some("excl.clear"));
        let more_btn = gtk::MenuButton::builder().icon_name("view-more-symbolic").menu_model(&more_menu).tooltip_text(tr!("More")).build();
        more_btn.add_css_class("flat");
        header.append(&add_btn);
        header.append(&more_btn);
        sites_group.set_header_suffix(Some(&header));

        let search = adw::EntryRow::builder().title(tr!("Search")).build();
        search.add_prefix(&gtk::Image::from_icon_name("system-search-symbolic"));
        sites_group.add(&search);

        // ---------------------------------------------------------------- apps (not supported)
        let apps_group = adw::PreferencesGroup::builder().title(tr!("Apps")).build();
        let apps_row = adw::ActionRow::builder()
            .title(tr!("App exclusions are not available on Linux"))
            .subtitle(tr!("AdGuard VPN CLI supports exclusions only for websites, IP addresses and subnets. To exclude an app's traffic, add the domains it uses."))
            .build();
        apps_row.add_prefix(&gtk::Image::from_icon_name("agvpn-info-symbolic"));
        apps_group.add(&apps_row);
        pref.add(&apps_group);

        let page = Rc::new(Self {
            widget,
            ui,
            general_check,
            selective_check,
            mode_handler: RefCell::new(None),
            sites: Rows::new(sites_group),
            search,
        });

        let weak = Rc::downgrade(&page);
        let actions = gio::SimpleActionGroup::new();
        let handlers: [(&str, fn(&Rc<ExclusionsPage>)); 5] = [
            ("add", Self::add_dialog),
            ("services", Self::services_dialog),
            ("import", Self::import),
            ("export", Self::export),
            ("clear", Self::clear),
        ];
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
        page.widget.insert_action_group("excl", Some(&actions));

        let handler = page.general_check.connect_toggled({
            let weak = weak.clone();
            move |check| {
                if let Some(page) = weak.upgrade() {
                    let mode = if check.is_active() { "general" } else { "selective" };
                    page.ui.ctl.set_exclusion_mode(mode);
                }
            }
        });
        *page.mode_handler.borrow_mut() = Some(handler);
        page.search.connect_changed({
            let weak = weak.clone();
            move |_| {
                if let Some(page) = weak.upgrade() {
                    page.rebuild();
                }
            }
        });
        page.ui.ctl.signals.exclusions_changed.connect({
            let weak = weak.clone();
            move |_| {
                if let Some(page) = weak.upgrade() {
                    page.rebuild();
                }
            }
        });
        page.rebuild();
        page
    }

    // ================================================================== state

    fn mode(&self) -> String {
        self.ui.ctl.exclusion_mode()
    }

    fn services_for_mode(&self) -> Vec<(String, Vec<String>)> {
        self.ui.ctl.settings.services(&self.mode())
    }

    fn set_services_for_mode(&self, services: &[(String, Vec<String>)]) {
        self.ui.ctl.settings.set_services(&self.mode(), services);
    }

    fn rebuild(self: &Rc<Self>) {
        let mode = self.mode();
        if let Some(handler) = self.mode_handler.borrow().as_ref() {
            self.general_check.block_signal(handler);
            self.general_check.set_active(mode == "general");
            self.selective_check.set_active(mode == "selective");
            self.general_check.unblock_signal(handler);
        }
        self.sites.clear();

        let mut items = self.ui.ctl.exclusions_for(&mode);
        if mode == "general" {
            // Subnets of the general list are managed in Advanced settings → Subnet exclusions.
            items.retain(|i| !(i.contains('/') && is_ip_like(i)));
        }
        let present: HashSet<&String> = items.iter().collect();
        let query = self.search.text().trim().to_lowercase();

        // Services first
        let mut claimed: HashSet<String> = HashSet::new();
        let mut shown = 0;
        for (service_id, patterns) in self.services_for_mode() {
            let active: Vec<String> = patterns.into_iter().filter(|p| present.contains(p)).collect();
            if active.is_empty() {
                continue;
            }
            claimed.extend(active.iter().cloned());
            let name = service_by_id(&service_id).map(|s| s.name.to_string()).unwrap_or_else(|| service_id.clone());
            if !query.is_empty() && !name.to_lowercase().contains(&query) && !active.iter().any(|p| p.contains(&query)) {
                continue;
            }
            self.add_service_row(&service_id, &name, active);
            shown += 1;
        }

        let rest: Vec<String> = items.iter().filter(|i| !claimed.contains(*i)).cloned().collect();
        for (base, has_domain, has_sub) in group_domains(&rest) {
            if !query.is_empty() && !base.contains(&query) {
                continue;
            }
            self.add_domain_row(&base, has_domain, has_sub);
            shown += 1;
        }

        let total = items.len() as i64;
        let group = self.sites.group();
        let desc = if mode == "general" {
            group.set_title(&tr!("Websites excluded from VPN"));
            if total == 0 {
                tr!("VPN is active for all websites")
            } else {
                ngettext("VPN is active for all websites except {n}", "VPN is active for all websites except {n}", total)
            }
        } else {
            group.set_title(&tr!("Websites that use VPN"));
            if total == 0 {
                tr!("List is empty. VPN is inactive for all websites")
            } else {
                ngettext("VPN is active for {n} website only", "VPN is active for {n} websites only", total)
            }
        };
        group.set_description(Some(&desc));
        if shown == 0 && !query.is_empty() {
            self.sites.add(&adw::ActionRow::builder().title(tr!("Nothing found")).build());
        }
    }

    fn add_service_row(self: &Rc<Self>, service_id: &str, name: &str, patterns: Vec<String>) {
        let row = adw::ExpanderRow::builder()
            .title(escape(name))
            .subtitle(ngettext("{n} domain", "{n} domains", patterns.len() as i64))
            .build();
        let remove = flat_icon_button("user-trash-symbolic", &tr!("Remove"));
        {
            let (weak, id, patterns) = (Rc::downgrade(self), service_id.to_string(), patterns.clone());
            remove.connect_clicked(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.remove_service(&id, patterns.clone());
                }
            });
        }
        row.add_suffix(&remove);
        for pattern in patterns {
            let sub = adw::ActionRow::builder().title(escape(&pattern)).build();
            let btn = flat_icon_button("list-remove-symbolic", &tr!("Remove"));
            let ctl = self.ui.ctl.clone();
            btn.connect_clicked(move |_| ctl.remove_exclusions(vec![pattern.clone()], None, None));
            sub.add_suffix(&btn);
            row.add_row(&sub);
        }
        self.sites.add(&row);
    }

    fn add_domain_row(self: &Rc<Self>, base: &str, has_domain: bool, has_sub: bool) {
        let ip = is_ip_like(base);
        let subtitle = if ip {
            tr!("IP address or subnet")
        } else if has_domain && has_sub {
            tr!("Domain and all subdomains")
        } else if has_sub {
            tr!("All subdomains only")
        } else {
            tr!("Domain only")
        };
        let row = adw::ActionRow::builder().title(escape(base)).subtitle(subtitle).build();
        row.add_prefix(&gtk::Image::from_icon_name(if ip { "agvpn-network-symbolic" } else { "agvpn-globe-symbolic" }));
        if !ip {
            let switch = gtk::Switch::builder().active(has_sub).valign(gtk::Align::Center).tooltip_text(tr!("Include subdomains")).build();
            let (ctl, base) = (self.ui.ctl.clone(), base.to_string());
            switch.connect_active_notify(move |sw| {
                if sw.is_active() {
                    ctl.add_exclusions(vec![base.clone(), format!("*.{base}")], None, None);
                } else {
                    ctl.remove_exclusions(vec![format!("*.{base}")], None, None);
                }
            });
            row.add_suffix(&switch);
        }
        let remove = flat_icon_button("user-trash-symbolic", &tr!("Remove"));
        let mut patterns = Vec::new();
        if has_domain {
            patterns.push(base.to_string());
        }
        if has_sub {
            patterns.push(format!("*.{base}"));
        }
        let ctl = self.ui.ctl.clone();
        remove.connect_clicked(move |_| ctl.remove_exclusions(patterns.clone(), None, Some(tr!("Website removed"))));
        row.add_suffix(&remove);
        self.sites.add(&row);
    }

    // ================================================================== actions

    fn remove_service(&self, service_id: &str, patterns: Vec<String>) {
        let mut services = self.services_for_mode();
        services.retain(|(id, _)| id != service_id);
        self.set_services_for_mode(&services);
        self.ui.ctl.remove_exclusions(patterns, None, Some(tr!("Website or group of websites removed")));
    }

    fn add_dialog(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::new(
            Some(&tr!("Add website")),
            Some(&tr!("Enter a domain name, IP address or subnet in CIDR notation")),
        );
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        list.add_css_class("boxed-list");
        let entry = adw::EntryRow::builder().title(tr!("Domain or IP address")).build();
        let subdomains = adw::SwitchRow::builder().title(tr!("Include subdomains")).active(true).build();
        list.append(&entry);
        list.append(&subdomains);
        dialog.set_extra_child(Some(&list));
        dialog.add_response("cancel", &tr!("Cancel"));
        dialog.add_response("add", &tr!("Add"));
        dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("add"));
        dialog.set_close_response("cancel");

        let update = {
            let (dialog, subdomains) = (dialog.clone(), subdomains.clone());
            move |entry: &adw::EntryRow| {
                let value = normalize_entry(&entry.text());
                dialog.set_response_enabled("add", value.is_some());
                subdomains.set_sensitive(value.as_ref().is_some_and(|v| !is_ip_like(v) && !v.starts_with("*.")));
                entry.remove_css_class("error");
                if !entry.text().trim().is_empty() && value.is_none() {
                    entry.add_css_class("error");
                }
            }
        };
        update(&entry);
        entry.connect_changed(move |e| update(e));

        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            let Some(page) = weak.upgrade() else { return };
            if response != "add" {
                return;
            }
            let Some(value) = normalize_entry(&entry.text()) else { return };
            let existing: HashSet<String> = page.ui.ctl.exclusions_for(&page.mode()).into_iter().collect();
            let mut items = vec![value.clone()];
            if subdomains.is_active() && subdomains.is_sensitive() {
                items.push(format!("*.{value}"));
            }
            items.retain(|i| !existing.contains(i));
            if items.is_empty() {
                page.ui.toast(&tr!("This website is already in the list"));
                return;
            }
            page.ui.ctl.add_exclusions(items, None, Some(tr!("Website added")));
        });
        dialog.present(Some(&self.ui.window));
    }

    fn services_dialog(self: &Rc<Self>) {
        ServicesDialog::present(self.clone());
    }

    fn clear(self: &Rc<Self>) {
        let ctl = self.ui.ctl.clone();
        confirm(
            &self.ui.window,
            &tr!("Clear list?"),
            &tr!("All websites will be removed from the list for the current mode"),
            &tr!("Clear"),
            true,
            move || ctl.clear_exclusions(),
        );
    }

    fn import(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(&tr!("Text files")));
        filter.add_suffix("txt");
        filter.add_mime_type("text/plain");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder().title(tr!("Import exclusions")).filters(&filters).build();
        let weak = Rc::downgrade(self);
        dialog.open(Some(&self.ui.window), gio::Cancellable::NONE, move |res| {
            let (Some(page), Ok(file)) = (weak.upgrade(), res) else { return };
            let Ok((data, _)) = file.load_contents(gio::Cancellable::NONE) else { return };
            let text = String::from_utf8_lossy(&data);
            let existing: HashSet<String> = page.ui.ctl.exclusions_for(&page.mode()).into_iter().collect();
            let mut seen = HashSet::new();
            let items: Vec<String> = text
                .lines()
                .filter_map(|line| {
                    let line = line.split('#').next().unwrap_or("").trim();
                    if line.is_empty() { None } else { normalize_entry(line) }
                })
                .filter(|i| seen.insert(i.clone()) && !existing.contains(i))
                .collect();
            if items.is_empty() {
                page.ui.toast(&tr!("Nothing to import"));
                return;
            }
            let message = ngettext("Imported {n} entry", "Imported {n} entries", items.len() as i64);
            page.ui.ctl.add_exclusions(items, None, Some(message));
        });
    }

    fn export(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder()
            .title(tr!("Export exclusions"))
            .initial_name(format!("exclusions-{}.txt", self.mode()))
            .build();
        let weak = Rc::downgrade(self);
        dialog.save(Some(&self.ui.window), gio::Cancellable::NONE, move |res| {
            let (Some(page), Ok(file)) = (weak.upgrade(), res) else { return };
            let data = page.ui.ctl.exclusions_for(&page.mode()).join("\n") + "\n";
            match file.replace_contents(data.as_bytes(), None, false, gio::FileCreateFlags::REPLACE_DESTINATION, gio::Cancellable::NONE) {
                Ok(_) => page.ui.toast(&tr!("Exclusions exported")),
                Err(err) => page.ui.show_error(&tr!("Export exclusions"), err.message()),
            }
        });
    }
}

/// Pick popular services; each adds a group of domains (with subdomains).
struct ServicesDialog;

impl ServicesDialog {
    fn present(page: Rc<ExclusionsPage>) {
        let dialog = adw::Dialog::builder().title(tr!("Popular services")).content_width(460).content_height(640).build();
        let services = page.services_for_mode();
        let present: HashSet<String> = page.ui.ctl.exclusions_for(&page.mode()).into_iter().collect();

        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::builder().show_end_title_buttons(false).show_start_title_buttons(false).build();
        let cancel = gtk::Button::with_label(&tr!("Cancel"));
        let save = gtk::Button::with_label(&tr!("Save"));
        save.add_css_class("suggested-action");
        header.pack_start(&cancel);
        header.pack_end(&save);
        toolbar.add_top_bar(&header);
        let search = gtk::SearchEntry::builder().placeholder_text(tr!("Search")).margin_start(12).margin_end(12).margin_bottom(6).build();
        toolbar.add_top_bar(&search);
        let pref = adw::PreferencesPage::new();
        toolbar.set_content(Some(&pref));
        dialog.set_child(Some(&toolbar));

        let mut groups: Vec<(&str, adw::PreferencesGroup)> = Vec::new();
        let mut rows: Vec<(adw::ActionRow, String, adw::PreferencesGroup)> = Vec::new();
        let mut checks: Vec<(&'static str, gtk::CheckButton, bool)> = Vec::new();
        for service in SERVICES {
            let group = match groups.iter().find(|(c, _)| *c == service.category) {
                Some((_, g)) => g.clone(),
                None => {
                    let g = adw::PreferencesGroup::builder().title(tr!(service.category)).build();
                    pref.add(&g);
                    groups.push((service.category, g.clone()));
                    g
                }
            };
            let active = services.iter().any(|(id, patterns)| id == service.id && patterns.iter().any(|p| present.contains(p)));
            let check = gtk::CheckButton::builder().active(active).build();
            let mut subtitle = service.domains.iter().take(3).copied().collect::<Vec<_>>().join(", ");
            if service.domains.len() > 3 {
                subtitle.push('…');
            }
            let row = adw::ActionRow::builder().title(escape(service.name)).subtitle(subtitle).activatable_widget(&check).build();
            row.add_prefix(&check);
            group.add(&row);
            checks.push((service.id, check, active));
            rows.push((row, format!("{} {}", service.name.to_lowercase(), service.domains.join(" ")), group));
        }

        let all_groups: Vec<adw::PreferencesGroup> = groups.into_iter().map(|(_, g)| g).collect();
        search.connect_search_changed(move |search| {
            let query = search.text().trim().to_lowercase();
            let mut visible_groups: Vec<adw::PreferencesGroup> = Vec::new();
            for (row, haystack, group) in &rows {
                let visible = query.is_empty() || haystack.contains(&query);
                row.set_visible(visible);
                if visible && !visible_groups.contains(group) {
                    visible_groups.push(group.clone());
                }
            }
            for group in &all_groups {
                group.set_visible(visible_groups.contains(group));
            }
        });

        {
            let dialog = dialog.clone();
            cancel.connect_clicked(move |_| {
                dialog.close();
            });
        }
        {
            let (dialog, page) = (dialog.clone(), page.clone());
            save.connect_clicked(move |_| {
                let mut services = page.services_for_mode();
                let present: HashSet<String> = page.ui.ctl.exclusions_for(&page.mode()).into_iter().collect();
                let (mut to_add, mut to_remove) = (Vec::new(), Vec::new());
                let (mut added, mut removed) = (0, 0);
                for (id, check, initial) in &checks {
                    let want = check.is_active();
                    if want == *initial {
                        continue;
                    }
                    let patterns = service_patterns(service_by_id(id).map(|s| s.domains).unwrap_or(&[]));
                    if want {
                        to_add.extend(patterns.iter().filter(|p| !present.contains(*p)).cloned());
                        services.retain(|(sid, _)| sid != id);
                        services.push((id.to_string(), patterns));
                        added += 1;
                    } else {
                        let stored = services.iter().position(|(sid, _)| sid == id).map(|pos| services.remove(pos).1);
                        to_remove.extend(stored.unwrap_or(patterns).into_iter().filter(|p| present.contains(p)));
                        removed += 1;
                    }
                }
                page.set_services_for_mode(&services);
                let message = tr!("Added: {added}, deleted: {deleted}", added = added, deleted = removed);
                if !to_remove.is_empty() {
                    let msg = if to_add.is_empty() { Some(message.clone()) } else { None };
                    page.ui.ctl.remove_exclusions(to_remove, None, msg);
                }
                if !to_add.is_empty() {
                    page.ui.ctl.add_exclusions(to_add, None, Some(message));
                }
                dialog.close();
            });
        }
        dialog.present(Some(&page.ui.window));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize() {
        assert_eq!(normalize_entry("https://www.Example.com/path?q=1").as_deref(), Some("www.example.com"));
        assert_eq!(normalize_entry("example.com").as_deref(), Some("example.com"));
        assert_eq!(normalize_entry("*.example.com").as_deref(), Some("*.example.com"));
        assert_eq!(normalize_entry("10.0.0.0/8").as_deref(), Some("10.0.0.0/8"));
        assert_eq!(normalize_entry("192.168.1.10").as_deref(), Some("192.168.1.10"));
        assert_eq!(normalize_entry("192.168.1.10:8080").as_deref(), Some("192.168.1.10:8080"));
        assert_eq!(normalize_entry("яндекс.рф").as_deref(), Some("xn--d1acpjx3f.xn--p1ai"));
        assert_eq!(normalize_entry("not a domain"), None);
        assert_eq!(normalize_entry(""), None);
    }

    #[test]
    fn group() {
        let items: Vec<String> = ["a.com", "*.a.com", "*.b.com", "c.com"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            group_domains(&items),
            vec![("a.com".into(), true, true), ("b.com".into(), false, true), ("c.com".into(), true, false)]
        );
    }

    #[test]
    fn subnets() {
        assert!(is_subnet("192.168.1.0/24"));
        assert!(!is_subnet("192.168.1.0"));
        assert!(!is_subnet("192.168.1.0/33"));
        assert_eq!(normalize_cidr("192.168.1.7/24").as_deref(), Some("192.168.1.0/24"));
        assert_eq!(normalize_cidr("fd00::1/8").as_deref(), Some("fd00::/8"));
    }
}
