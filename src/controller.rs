//! Central application state: talks to the CLI and notifies the UI through signals.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gtk::glib;

use crate::cli::Cli;
use crate::parsers::{self, CliConfig, License, Location, State, Status};
use crate::settings::Settings;
use crate::stats::Stats;
use crate::tr;
use crate::util::{now_secs, CmdResult, Signal, Worker};

const POLL_FAST: u32 = 3;
const LOCATIONS_TTL: f64 = 10.0 * 60.0;
const LICENSE_TTL: f64 = 30.0 * 60.0;

/// Settings that only take effect after reconnecting.
const RECONNECT_KEYS: &[&str] = &[
    "set-mode", "set-dns", "set-protocol", "set-post-quantum", "set-socks-port", "set-socks-host",
    "set-tun-routing-mode", "set-change-system-dns", "set-bound-if-override",
];

pub fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id").map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Human-readable explanation for a failed CLI call.
pub fn error_message(res: &CmdResult) -> String {
    let text = res.text();
    let low = text.to_lowercase();
    if res.code == 126 || low.contains("request dismissed") || low.contains("authentication failed")
        || low.contains("incorrect password attempt")
    {
        return tr!("Authorization was cancelled");
    }
    let table: &[(&str, &str)] = &[
        ("too many devices", "Connections limit reached. Disconnect AdGuard VPN on another device"),
        ("no location with the specified", "Location not found"),
        ("location unavailable", "Location not responding. Please try again or select another one"),
        ("none of the endpoints are available", "Location not responding. Please try again or select another one"),
        ("does not support the ipv6", "This location does not support IPv6"),
        ("please log in", "You are not logged in"),
        ("session has expired", "Your session has expired. Please log in again"),
        ("/dev/net/tun does not exist", "TUN device is not available. Run `sudo modprobe tun` and try again"),
        ("tun device is not available", "TUN device is not available. Run `sudo modprobe tun` and try again"),
        ("already used", "The port is already used by another app. Set another port in Advanced settings"),
        ("address already in use", "The port is already used by another app. Set another port in Advanced settings"),
        ("traffic limit", "You've reached the free data limit"),
        ("neither pkexec nor sudo", "Neither pkexec nor sudo is available on the host system"),
        ("not in the sudoers", "Your user is not allowed to use sudo"),
        ("too many requests", "Too many requests. Please try again in a few minutes"),
        ("adguardvpn-cli not found", "AdGuard VPN CLI is not installed"),
        ("invalid upstream", "Invalid DNS server address"),
        ("nft (nftables) is required", "Kill Switch requires nftables (the `nft` command) on the host"),
        ("timed out", "The operation timed out"),
    ];
    for (marker, message) in table {
        if low.contains(marker) {
            return tr!(message);
        }
    }
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_string())
        .unwrap_or_else(|| tr!("Something went wrong"))
}

#[derive(Default)]
pub struct Signals {
    pub status_changed: Signal<()>,
    pub locations_changed: Signal<()>,
    pub license_changed: Signal<()>,
    pub config_changed: Signal<()>,
    pub exclusions_changed: Signal<()>,
    pub cli_changed: Signal<()>,
    /// title, detail
    pub error: Signal<(String, String)>,
    /// message shown as a toast
    pub toast: Signal<String>,
    pub needs_reconnect: Signal<()>,
    /// previous state, new state
    pub state_transition: Signal<(State, State)>,
}

pub struct Exclusions {
    pub mode: String,
    pub general: Vec<String>,
    pub selective: Vec<String>,
}

impl Exclusions {
    pub fn for_mode(&self, mode: &str) -> &[String] {
        if mode == "selective" { &self.selective } else { &self.general }
    }
}

pub struct Controller {
    pub settings: Rc<Settings>,
    pub stats: RefCell<Stats>,
    pub cli: Arc<Cli>,
    pub worker: Worker,
    pub signals: Signals,

    pub status: RefCell<Status>,
    /// Connecting / Disconnecting while a privileged call runs.
    pub busy: Cell<Option<State>>,
    busy_token: Cell<u64>,
    pub user_disconnected: Cell<bool>,
    pub last_error: RefCell<String>,

    pub locations: RefCell<Vec<Location>>,
    pub locations_error: RefCell<String>,
    pub locations_loading: Cell<bool>,
    locations_at: Cell<f64>,

    pub license: RefCell<License>,
    pub logged_in: Cell<Option<bool>>,
    license_at: Cell<f64>,

    pub config: RefCell<CliConfig>,
    pub exclusions: RefCell<Exclusions>,

    poll_source: RefCell<Option<glib::SourceId>>,
    status_pending: Cell<bool>,
    pub connected_since: Cell<Option<f64>>,
}

impl Controller {
    pub fn new(settings: Rc<Settings>) -> Rc<Self> {
        Rc::new(Self {
            settings,
            stats: RefCell::new(Stats::new()),
            cli: Cli::new(),
            worker: Worker::new("cli"),
            signals: Signals::default(),
            status: RefCell::new(Status::default()),
            busy: Cell::new(None),
            busy_token: Cell::new(0),
            user_disconnected: Cell::new(false),
            last_error: RefCell::new(String::new()),
            locations: RefCell::new(Vec::new()),
            locations_error: RefCell::new(String::new()),
            locations_loading: Cell::new(false),
            locations_at: Cell::new(0.0),
            license: RefCell::new(License::default()),
            logged_in: Cell::new(None),
            license_at: Cell::new(0.0),
            config: RefCell::new(CliConfig::default()),
            exclusions: RefCell::new(Exclusions { mode: "general".into(), general: Vec::new(), selective: Vec::new() }),
            poll_source: RefCell::new(None),
            status_pending: Cell::new(false),
            connected_since: Cell::new(None),
        })
    }

    // ================================================================== lifecycle

    pub fn start(self: &Rc<Self>) {
        if !self.cli.available() {
            self.signals.cli_changed.emit(());
            return;
        }
        if self.cli.version().is_empty() {
            let cli = self.cli.clone();
            let this = self.clone();
            self.worker.submit(move || cli.load_version(), move |_| this.signals.cli_changed.emit(()));
        }
        self.refresh_config();
        self.refresh_exclusions();
        self.refresh_license();
        self.refresh_status();
        self.refresh_locations();
        if self.poll_source.borrow().is_none() {
            let weak = Rc::downgrade(self);
            let id = glib::timeout_add_seconds_local(POLL_FAST, move || match weak.upgrade() {
                Some(this) => {
                    this.poll();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
            *self.poll_source.borrow_mut() = Some(id);
        }
    }

    pub fn stop(&self) {
        if let Some(id) = self.poll_source.borrow_mut().take() {
            id.remove();
        }
        let mut stats = self.stats.borrow_mut();
        stats.tick(false, "", "");
        stats.flush();
    }

    pub fn reload_cli(self: &Rc<Self>, callback: Option<Box<dyn FnOnce()>>) {
        let cli = self.cli.clone();
        let this = self.clone();
        self.worker.submit(
            move || {
                cli.reload_binary();
                cli.load_version();
            },
            move |_| {
                this.signals.cli_changed.emit(());
                this.start();
                if let Some(cb) = callback {
                    cb();
                }
            },
        );
    }

    fn poll(self: &Rc<Self>) {
        let now = now_secs();
        if !self.status_pending.get() && self.busy.get().is_none() {
            self.refresh_status();
        }
        let logged_in = self.logged_in.get() == Some(true);
        if logged_in && now - self.locations_at.get() > LOCATIONS_TTL && !self.locations_loading.get() {
            self.refresh_locations();
        }
        if logged_in && now - self.license_at.get() > LICENSE_TTL {
            self.refresh_license();
        }
    }

    // ================================================================== status

    pub fn state(&self) -> State {
        self.busy.get().unwrap_or(self.status.borrow().state)
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.status.borrow().state, State::Connected | State::Reconnecting)
    }

    fn set_ks_active(&self, active: bool) {
        // nftables rules do not survive a reboot, so the flag is bound to the boot id.
        if active {
            self.settings.set("kill_switch_active", boot_id());
        } else {
            self.settings.set("kill_switch_active", false);
        }
    }

    pub fn kill_switch_rules_present(&self) -> bool {
        match self.settings.get("kill_switch_active") {
            serde_json::Value::Bool(b) => b,
            serde_json::Value::String(s) => !s.is_empty() && s == boot_id(),
            _ => false,
        }
    }

    pub fn kill_switch_blocking(&self) -> bool {
        self.kill_switch_rules_present() && self.status.borrow().state == State::Disconnected && self.busy.get().is_none()
    }

    pub fn refresh_status(self: &Rc<Self>) {
        self.status_pending.set(true);
        let cli = self.cli.clone();
        let this = self.clone();
        self.worker.submit(move || cli.status(), move |st| {
            this.status_pending.set(false);
            this.apply_status(st);
        });
    }

    fn apply_status(&self, st: Status) {
        let prev = self.status.borrow().state;
        let state = st.state;
        match state {
            State::NotLoggedIn => self.set_logged_in(false),
            State::Connected | State::Connecting | State::Reconnecting => self.set_logged_in(true),
            _ => {}
        }
        if matches!(state, State::Connected | State::Reconnecting) {
            if self.connected_since.get().is_none() {
                self.connected_since.set(Some(now_secs()));
            }
        } else {
            self.connected_since.set(None);
        }
        if self.settings.bool("stats_enabled") {
            let title = self.location_title(&st.location);
            self.stats.borrow_mut().tick(state == State::Connected, &st.interface, &title);
        }
        *self.status.borrow_mut() = st;
        if prev != state {
            self.signals.state_transition.emit((prev, state));
        }
        self.signals.status_changed.emit(());
    }

    fn set_logged_in(&self, value: bool) {
        if self.logged_in.get() != Some(value) {
            self.logged_in.set(Some(value));
            if !value {
                *self.license.borrow_mut() = License::default();
            }
            self.signals.license_changed.emit(());
        }
    }

    /// Matches the name printed by `status` against the locations list.
    pub fn find_location(&self, name: &str) -> Option<Location> {
        if name.is_empty() {
            return None;
        }
        let low = name.to_lowercase().replace(&parsers::VIRTUAL_SUFFIX.to_lowercase(), "");
        let low = low.trim();
        let locations = self.locations.borrow();
        locations
            .iter()
            .find(|l| l.city.to_lowercase() == low || l.connect_arg().to_lowercase() == low)
            .or_else(|| locations.iter().find(|l| l.country.to_lowercase() == low || l.iso.to_lowercase() == low))
            .cloned()
    }

    pub fn location_title(&self, name: &str) -> String {
        if let Some(loc) = self.find_location(name) {
            return loc.city;
        }
        let has_letters = name.chars().any(char::is_alphabetic);
        if has_letters && name.chars().all(|c| !c.is_lowercase()) {
            title_case(name)
        } else {
            name.to_string()
        }
    }

    pub fn current_location(&self) -> Option<Location> {
        let name = self.status.borrow().location.clone();
        self.find_location(&name)
    }

    // ================================================================== connect

    pub fn connect_vpn(self: &Rc<Self>, location: Option<Location>, fastest: bool) {
        if self.busy.get().is_some() {
            return;
        }
        let mut location = location;
        let mut fastest = fastest;
        if location.is_none() && !fastest {
            let last = self.settings.string("last_location");
            if last.is_empty() {
                fastest = true;
            } else {
                location = Some(
                    self.locations
                        .borrow()
                        .iter()
                        .find(|l| l.connect_arg() == last)
                        .cloned()
                        .unwrap_or_else(|| Location::named(&last)),
                );
            }
        }
        self.busy.set(Some(State::Connecting));
        let token = self.busy_token.get() + 1;
        self.busy_token.set(token);
        self.user_disconnected.set(false);
        self.last_error.borrow_mut().clear();
        self.signals.status_changed.emit(());

        let allow_lan = self.settings.bool("kill_switch_allow_lan");
        let ip_mode = self.settings.string("ip_mode");
        let requested_ks = self.settings.bool("kill_switch") && self.config.borrow().mode == "tun";
        let arg = location.as_ref().map(|l| l.connect_arg().to_string());
        let cli = self.cli.clone();
        let this = self.clone();
        self.worker.submit(
            move || {
                let res = cli.connect(arg.as_deref(), fastest, &ip_mode, requested_ks, allow_lan);
                (res, cli.status())
            },
            move |(res, st)| {
                if this.busy_token.get() == token {
                    this.busy.set(None);
                }
                if res.ok() {
                    if let Some(loc) = &location {
                        this.settings.set("last_location", loc.connect_arg());
                    }
                    // The helper always lifts old rules before connecting; this reflects the new state.
                    let enabled = res.out.contains("Kill Switch enabled");
                    this.set_ks_active(enabled);
                    if requested_ks && !enabled {
                        let text = res.text();
                        let detail = text
                            .lines()
                            .find(|l| l.contains("Kill Switch FAILED"))
                            .and_then(|l| l.split_once("FAILED:"))
                            .map(|(_, d)| d.trim().to_string())
                            .unwrap_or_default();
                        let message = if detail.is_empty() {
                            tr!("VPN is connected, but the firewall rules could not be installed")
                        } else {
                            error_message(&CmdResult::new(1, detail))
                        };
                        this.signals.error.emit((tr!("Kill Switch is not active"), message));
                    }
                }
                if !res.ok() && !matches!(st.state, State::Connected | State::Reconnecting) {
                    let message = error_message(&res);
                    *this.last_error.borrow_mut() = message.clone();
                    if res.code != 126 {
                        this.signals.error.emit((tr!("Can't connect"), message));
                    }
                }
                this.apply_status(st);
            },
        );
    }

    pub fn disconnect_vpn(self: &Rc<Self>, callback: Option<Box<dyn FnOnce(bool)>>) {
        if self.busy.get() == Some(State::Disconnecting) {
            return;
        }
        self.busy.set(Some(State::Disconnecting));
        let token = self.busy_token.get() + 1;
        self.busy_token.set(token);
        self.user_disconnected.set(true);
        self.signals.status_changed.emit(());

        let cli = self.cli.clone();
        let this = self.clone();
        self.worker.submit(
            move || (cli.disconnect(), cli.status()),
            move |(res, st)| {
                if this.busy_token.get() == token {
                    this.busy.set(None);
                }
                let ok = res.ok() || st.state == State::Disconnected;
                if ok {
                    this.set_ks_active(false);
                } else if res.code != 126 {
                    this.signals.error.emit((tr!("Can't disconnect"), error_message(&res)));
                }
                this.apply_status(st);
                this.signals.status_changed.emit(());
                if let Some(cb) = callback {
                    cb(ok);
                }
            },
        );
    }

    pub fn toggle(self: &Rc<Self>) {
        if self.state().is_active() {
            self.disconnect_vpn(None);
        } else {
            self.connect_vpn(None, false);
        }
    }

    pub fn set_kill_switch(self: &Rc<Self>, enabled: bool) {
        self.settings.set("kill_switch", enabled);
        if !self.is_connected() && !(!enabled && self.kill_switch_rules_present()) {
            return;
        }
        if enabled && self.config.borrow().mode != "tun" {
            self.signals.toast.emit(tr!("Kill Switch works only in VPN (TUN) mode"));
            return;
        }
        let allow_lan = self.settings.bool("kill_switch_allow_lan");
        let cli = self.cli.clone();
        let this = self.clone();
        self.worker.submit(
            move || if enabled { cli.kill_switch_on(allow_lan) } else { cli.kill_switch_off() },
            move |res| {
                if !res.ok() {
                    if res.code == 126 {
                        this.settings.set("kill_switch", !enabled);
                        return;
                    }
                    this.signals.error.emit((tr!("Kill Switch"), error_message(&res)));
                    return;
                }
                this.set_ks_active(enabled);
                this.signals.status_changed.emit(());
            },
        );
    }

    /// Removes the blocking rules left after an unexpected disconnect.
    pub fn release_kill_switch(self: &Rc<Self>) {
        let cli = self.cli.clone();
        let this = self.clone();
        self.worker.submit(
            move || cli.kill_switch_off(),
            move |res| {
                if !res.ok() {
                    if res.code != 126 {
                        this.signals.error.emit((tr!("Kill Switch"), error_message(&res)));
                    }
                    return;
                }
                this.set_ks_active(false);
                this.signals.status_changed.emit(());
            },
        );
    }

    // ================================================================== locations

    pub fn refresh_locations(self: &Rc<Self>) {
        if self.locations_loading.get() {
            return;
        }
        self.locations_loading.set(true);
        self.signals.locations_changed.emit(());
        let cli = self.cli.clone();
        let this = self.clone();
        self.worker.submit(
            move || cli.locations(),
            move |(locs, res)| {
                this.locations_loading.set(false);
                this.locations_at.set(now_secs());
                if !locs.is_empty() {
                    *this.locations.borrow_mut() = locs;
                    this.locations_error.borrow_mut().clear();
                    this.set_logged_in(true);
                } else {
                    if res.text().to_lowercase().contains("log in") {
                        this.set_logged_in(false);
                    }
                    *this.locations_error.borrow_mut() = error_message(&res);
                }
                this.signals.locations_changed.emit(());
                this.signals.status_changed.emit(());
            },
        );
    }

    pub fn fastest_locations(&self, count: usize) -> Vec<Location> {
        let mut with_ping: Vec<Location> = self.locations.borrow().iter().filter(|l| l.ping.is_some()).cloned().collect();
        with_ping.sort_by_key(|l| l.ping);
        with_ping.truncate(count);
        with_ping
    }

    pub fn is_saved(&self, loc: &Location) -> bool {
        self.settings.strings("saved_locations").contains(&loc.key())
    }

    pub fn set_saved(&self, loc: &Location, saved: bool) {
        let mut keys = self.settings.strings("saved_locations");
        let key = loc.key();
        if saved && !keys.contains(&key) {
            keys.push(key);
        } else if !saved {
            keys.retain(|k| k != &key);
        }
        self.settings.set("saved_locations", keys);
        self.signals.locations_changed.emit(());
    }

    pub fn saved_locations(&self) -> Vec<Location> {
        let keys = self.settings.strings("saved_locations");
        self.locations.borrow().iter().filter(|l| keys.contains(&l.key())).cloned().collect()
    }

    // ================================================================== account

    pub fn refresh_license(self: &Rc<Self>) {
        let cli = self.cli.clone();
        let this = self.clone();
        self.worker.submit(
            move || {
                let mut lic = cli.license();
                if lic.logged_in && lic.email.is_empty() {
                    lic.email = parsers::parse_email(&cli.run(&["status"], std::time::Duration::from_secs(20)).out);
                }
                lic
            },
            move |lic| {
                this.license_at.set(now_secs());
                this.logged_in.set(Some(lic.logged_in));
                *this.license.borrow_mut() = lic;
                this.signals.license_changed.emit(());
            },
        );
    }

    pub fn on_logged_in(self: &Rc<Self>) {
        self.logged_in.set(Some(true));
        self.license_at.set(0.0);
        self.start();
        self.signals.license_changed.emit(());
    }

    pub fn logout(self: &Rc<Self>) {
        let cli = self.cli.clone();
        let connected = self.is_connected();
        let this = self.clone();
        self.worker.submit(
            move || {
                if connected {
                    cli.disconnect();
                }
                let res = cli.logout();
                (res, cli.status())
            },
            move |(_res, st)| {
                this.set_ks_active(false);
                this.locations.borrow_mut().clear();
                this.set_logged_in(false);
                this.apply_status(st);
                this.signals.locations_changed.emit(());
            },
        );
    }

    // ================================================================== config

    pub fn refresh_config(self: &Rc<Self>) {
        let cli = self.cli.clone();
        let this = self.clone();
        self.worker.submit(move || cli.config(), move |cfg| {
            *this.config.borrow_mut() = cfg;
            this.signals.config_changed.emit(());
        });
    }

    pub fn set_config(self: &Rc<Self>, key: &str, values: &[&str], quiet: bool, callback: Option<Box<dyn FnOnce(bool)>>) {
        let cli = self.cli.clone();
        let this = self.clone();
        let key = key.to_string();
        let values: Vec<String> = values.iter().map(|s| s.to_string()).collect();
        let job_key = key.clone();
        self.worker.submit(
            move || {
                let res = cli.set_config(&job_key, &values);
                let cfg = if res.text().contains("Current configuration") {
                    parsers::parse_config(&res.out)
                } else {
                    cli.config()
                };
                (res, cfg)
            },
            move |(res, cfg)| {
                *this.config.borrow_mut() = cfg;
                let ok = res.ok() && !res.text().to_lowercase().contains("invalid");
                if !ok && !quiet {
                    this.signals.error.emit((tr!("Can't apply the setting"), error_message(&res)));
                } else if ok && RECONNECT_KEYS.contains(&key.as_str()) && this.is_connected() {
                    this.signals.needs_reconnect.emit(());
                }
                this.signals.config_changed.emit(());
                if let Some(cb) = callback {
                    cb(ok);
                }
            },
        );
    }

    pub fn set_config_quiet(self: &Rc<Self>, key: &str, values: &[&str]) {
        self.set_config(key, values, true, None);
    }

    pub fn reconnect(self: &Rc<Self>) {
        let loc = self.current_location();
        let fastest = loc.is_none();
        self.connect_vpn(loc, fastest);
    }

    /// First-run defaults so the non-interactive `connect -y` never has to ask.
    pub fn apply_initial_config(self: &Rc<Self>, telemetry: bool) {
        let onoff = if telemetry { "on" } else { "off" };
        self.set_config_quiet("set-show-hints", &["off"]);
        self.set_config_quiet("set-telemetry", &[onoff]);
        self.set_config_quiet("set-crash-reporting", &[onoff]);
        self.set_config_quiet("set-show-notifications", &["off"]);
        let (routing, sys_dns) = {
            let cfg = self.config.borrow();
            (cfg.routing_mode.clone(), cfg.change_system_dns)
        };
        if routing.is_empty() {
            self.set_config_quiet("set-tun-routing-mode", &["auto"]);
        }
        if sys_dns.is_none() {
            self.set_config_quiet("set-change-system-dns", &["on"]);
        }
    }

    pub fn ensure_cli_defaults(self: &Rc<Self>) {
        let cli = self.cli.clone();
        let this = self.clone();
        self.worker.submit(
            move || {
                let cfg = cli.config();
                let set = |key: &str, value: &str| {
                    cli.set_config(key, &[value.to_string()]);
                };
                if cfg.show_hints {
                    set("set-show-hints", "off");
                }
                if cfg.show_notifications {
                    set("set-show-notifications", "off");
                }
                if cfg.routing_mode.is_empty() {
                    set("set-tun-routing-mode", "auto");
                }
                if cfg.change_system_dns.is_none() {
                    set("set-change-system-dns", "on");
                }
                if cfg.telemetry.is_none() {
                    set("set-telemetry", "off");
                }
                if cfg.crash_reporting.is_none() {
                    set("set-crash-reporting", "off");
                }
                cli.config()
            },
            move |cfg| {
                *this.config.borrow_mut() = cfg;
                this.signals.config_changed.emit(());
            },
        );
    }

    // ================================================================== exclusions

    pub fn refresh_exclusions(self: &Rc<Self>) {
        let cli = self.cli.clone();
        let this = self.clone();
        self.worker.submit(
            move || (cli.exclusion_mode(), cli.exclusions("general"), cli.exclusions("selective")),
            move |(mode, general, selective)| {
                *this.exclusions.borrow_mut() = Exclusions { mode, general, selective };
                this.signals.exclusions_changed.emit(());
            },
        );
    }

    pub fn exclusion_mode(&self) -> String {
        self.exclusions.borrow().mode.clone()
    }

    pub fn exclusions_for(&self, mode: &str) -> Vec<String> {
        self.exclusions.borrow().for_mode(mode).to_vec()
    }

    fn exclusions_job(
        self: &Rc<Self>,
        job: impl FnOnce(&Cli) -> CmdResult + Send + 'static,
        message: Option<String>,
        callback: Option<Box<dyn FnOnce(bool)>>,
    ) {
        let cli = self.cli.clone();
        let this = self.clone();
        self.worker.submit(
            move || {
                let res = job(&cli);
                (res, cli.exclusion_mode(), cli.exclusions("general"), cli.exclusions("selective"))
            },
            move |(res, mode, general, selective)| {
                *this.exclusions.borrow_mut() = Exclusions { mode, general, selective };
                let ok = res.ok() && !res.text().to_lowercase().contains("invalid");
                if !ok {
                    this.signals.error.emit((tr!("Exclusions"), error_message(&res)));
                } else if let Some(message) = message {
                    this.signals.toast.emit(message);
                }
                this.signals.exclusions_changed.emit(());
                if let Some(cb) = callback {
                    cb(ok);
                }
            },
        );
    }

    pub fn set_exclusion_mode(self: &Rc<Self>, mode: &str) {
        if mode != self.exclusion_mode() {
            let mode = mode.to_string();
            self.exclusions_job(move |cli| cli.set_exclusion_mode(&mode), None, None);
        }
    }

    pub fn add_exclusions(self: &Rc<Self>, items: Vec<String>, mode: Option<&str>, message: Option<String>) {
        let mode = mode.map(String::from).unwrap_or_else(|| self.exclusion_mode());
        let items: Vec<String> = items.into_iter().filter(|i| !i.is_empty()).collect();
        if !items.is_empty() {
            self.exclusions_job(move |cli| cli.add_exclusions(&items, &mode), message, None);
        }
    }

    pub fn remove_exclusions(self: &Rc<Self>, items: Vec<String>, mode: Option<&str>, message: Option<String>) {
        let mode = mode.map(String::from).unwrap_or_else(|| self.exclusion_mode());
        let items: Vec<String> = items.into_iter().filter(|i| !i.is_empty()).collect();
        if !items.is_empty() {
            self.exclusions_job(move |cli| cli.remove_exclusions(&items, &mode), message, None);
        }
    }

    pub fn clear_exclusions(self: &Rc<Self>) {
        let mode = self.exclusion_mode();
        self.settings.set_services(&mode, &[]);
        self.exclusions_job(move |cli| cli.clear_exclusions(&mode), None, None);
    }

    pub fn replace_exclusions(self: &Rc<Self>, mode: &str, items: Vec<String>) {
        let mode = mode.to_string();
        self.exclusions_job(
            move |cli| {
                cli.clear_exclusions(&mode);
                if items.is_empty() { CmdResult::new(0, "") } else { cli.add_exclusions(&items, &mode) }
            },
            None,
            None,
        );
    }
}

/// "NEW YORK" -> "New York" (like Python's str.title()).
fn title_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_letter = false;
    for ch in text.chars() {
        if prev_letter {
            out.extend(ch.to_lowercase());
        } else {
            out.extend(ch.to_uppercase());
        }
        prev_letter = ch.is_alphabetic();
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn title_case() {
        assert_eq!(super::title_case("NEW YORK"), "New York");
        assert_eq!(super::title_case("SAO-PAULO"), "Sao-Paulo");
    }
}
