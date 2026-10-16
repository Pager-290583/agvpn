//! Parsers for the text output of adguardvpn-cli.
//!
//! The CLI has no machine-readable output, so everything here is based on the message
//! formats embedded in the official binary (v1.7.x), e.g.
//!     "Connected to {} in {} mode{}"  with suffix ", running on {iface}" / ", listening on {addr}"
//!     "{:<5} {:<20} {:<30} {:<10}"   for list-locations (ISO, COUNTRY, CITY, PING ESTIMATE)

use std::sync::LazyLock;

use regex::Regex;

use crate::util::strip_ansi;

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap()
}

// --------------------------------------------------------------------------- locations

pub const VIRTUAL_SUFFIX: &str = "(Virtual)";

#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub iso: String,
    pub country: String,
    pub city: String,
    pub ping: Option<u32>,
    pub is_virtual: bool,
}

impl Location {
    pub fn named(city: &str) -> Self {
        Self { iso: String::new(), country: String::new(), city: city.into(), ping: None, is_virtual: false }
    }

    pub fn key(&self) -> String {
        format!("{}:{}", self.iso, self.city).to_lowercase()
    }

    pub fn connect_arg(&self) -> &str {
        &self.city
    }
}

fn parse_ping(raw: &str) -> Option<u32> {
    static DIGITS: LazyLock<Regex> = LazyLock::new(|| re(r"\d+"));
    let value: u32 = DIGITS.find(raw)?.as_str().parse().ok()?;
    (value > 0).then_some(value)
}

fn make_location(iso: &str, country: &str, city: &str, ping_raw: &str) -> Option<Location> {
    let mut city = city.trim().to_string();
    let country = country.trim().to_string();
    if city.is_empty() {
        return None;
    }
    let mut is_virtual = false;
    if let Some(stripped) = city.strip_suffix(VIRTUAL_SUFFIX) {
        is_virtual = true;
        city = stripped.trim().to_string();
    }
    Some(Location { iso: iso.to_uppercase(), country, city, ping: parse_ping(ping_raw), is_virtual })
}

fn chars_slice(chars: &[char], start: usize, end: usize) -> String {
    let end = end.min(chars.len());
    if start >= end { String::new() } else { chars[start..end].iter().collect() }
}

/// Row whose country overflowed its 20-char column. The city is still padded to 30
/// characters and followed by one space and the ping, so it can be cut from the right.
fn parse_overflow_row(line: &str) -> Option<Location> {
    static ROW: LazyLock<Regex> = LazyLock::new(|| re(r"^([A-Za-z]{2})\s+(.*\S.*?) (\d+)\s*$"));
    let caps = ROW.captures(line)?;
    let iso = caps.get(1)?.as_str();
    let ping = caps.get(3)?.as_str();
    let body = line[iso.len()..].trim_start_matches(' ');
    let cut = body.rfind(&format!(" {ping}"))?;
    let body: Vec<char> = body[..cut].chars().collect();
    if body.len() < 32 {
        return None;
    }
    let n = body.len();
    make_location(iso, &chars_slice(&body, 0, n - 31), &chars_slice(&body, n - 30, n), ping)
}

pub fn parse_locations(output: &str) -> Vec<Location> {
    static ROW_START: LazyLock<Regex> = LazyLock::new(|| re(r"^([A-Za-z]{2})\s"));
    static SPACES: LazyLock<Regex> = LazyLock::new(|| re(r"\s{2,}"));
    let text = strip_ansi(output);
    let mut cols: Option<(usize, usize, Option<usize>)> = None;
    let mut result = Vec::new();
    for line in text.lines() {
        if cols.is_none() && line.contains("ISO") && line.contains("COUNTRY") && line.contains("CITY") {
            let chars: Vec<char> = line.chars().collect();
            let find = |word: &str| {
                let w: Vec<char> = word.chars().collect();
                chars.windows(w.len()).position(|win| win == w.as_slice())
            };
            let ping = find("PING").filter(|&p| p > 0);
            cols = Some((find("COUNTRY").unwrap_or(0), find("CITY").unwrap_or(0), ping));
            continue;
        }
        let Some(caps) = ROW_START.captures(line) else { continue };
        let iso = caps.get(1).unwrap().as_str();
        let chars: Vec<char> = line.chars().collect();
        let mut loc = None;
        if let Some((c_country, c_city, c_ping)) = cols {
            let end_city = c_ping.unwrap_or(chars.len());
            let space_at = |idx: usize| idx > 0 && chars.get(idx - 1) == Some(&' ');
            let aligned = chars.len() > c_city
                && space_at(c_country)
                && space_at(c_city)
                && c_ping.is_none_or(|p| chars.len() <= p || space_at(p));
            if aligned {
                let ping_raw = c_ping.map(|p| chars_slice(&chars, p, chars.len())).unwrap_or_default();
                loc = make_location(
                    iso,
                    &chars_slice(&chars, c_country, c_city),
                    &chars_slice(&chars, c_city, end_city),
                    &ping_raw,
                );
            }
        }
        if loc.is_none() {
            loc = parse_overflow_row(line);
        }
        if loc.is_none() {
            let tokens: Vec<&str> = SPACES.split(line.trim()).collect();
            if tokens.len() >= 3 {
                loc = make_location(tokens[0], tokens[1], tokens[2], tokens.get(3).copied().unwrap_or(""));
            }
        }
        if let Some(loc) = loc {
            result.push(loc);
        }
    }
    result
}

// --------------------------------------------------------------------------- status

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Disconnecting,
    NotLoggedIn,
    #[default]
    Unknown,
}

impl State {
    /// Connected, reconnecting or on the way there.
    pub fn is_active(self) -> bool {
        matches!(self, State::Connected | State::Reconnecting | State::Connecting)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub state: State,
    pub location: String,
    pub mode: String,
    pub interface: String,
    pub listen: String,
}

const LOC_MODE: &str = r"(?P<loc>.+?) in (?P<mode>\w+) mode(?:, (?P<kind>running on|listening on) (?P<where>\S+))?";

static CONNECTED_RE: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"(?i)(?:^|\s|Successfully )Connected to {LOC_MODE}")));
static CONNECTING_RE: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"(?i)(?:Reconnecting|Connecting) to {LOC_MODE}")));
static LOST_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"(?i)(?:Connection lost\. Waiting to reconnect|Network lost\. Waiting to connect) to {LOC_MODE}"
    ))
});

pub fn parse_status(output: &str) -> Status {
    let text = strip_ansi(output);
    let flat = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
    let low = flat.to_lowercase();
    let mut st = Status::default();

    if low.contains("not logged in") || low.contains("please log in") || low.contains("session has expired") {
        st.state = State::NotLoggedIn;
        return st;
    }

    for (regex, state) in [(&*LOST_RE, State::Reconnecting), (&*CONNECTED_RE, State::Connected), (&*CONNECTING_RE, State::Connecting)] {
        if let Some(caps) = regex.captures(&flat) {
            let mut state = state;
            if state == State::Connecting && caps[0].to_lowercase().starts_with("reconnecting") {
                state = State::Reconnecting;
            }
            st.state = state;
            st.location = caps["loc"].trim().to_string();
            st.mode = caps["mode"].to_uppercase();
            if let (Some(kind), Some(place)) = (caps.name("kind"), caps.name("where")) {
                let place = place.as_str().trim_end_matches(['.', ',']).to_string();
                if kind.as_str().eq_ignore_ascii_case("running on") {
                    st.interface = place;
                } else {
                    st.listen = place;
                }
            }
            return st;
        }
    }

    if low.contains("vpn active, but status unavailable") {
        st.state = State::Connected;
    } else if low.contains("vpn is disconnected") || low.contains("vpn stopped") {
        st.state = State::Disconnected;
    } else if low.contains("vpn is starting") {
        st.state = State::Connecting;
    }
    st
}

// --------------------------------------------------------------------------- license

#[derive(Debug, Clone, Default)]
pub struct License {
    pub logged_in: bool,
    pub email: String,
    pub premium: Option<bool>,
    pub devices: Option<u32>,
    pub renew_date: String,
    pub valid_until: String,
    pub traffic_left_gb: Option<f64>,
    pub traffic_exhausted: bool,
}

static EMAIL_RE: LazyLock<Regex> = LazyLock::new(|| re(r"Logged in as\s+(\S+)"));

pub fn parse_license(output: &str) -> License {
    static PLAN: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)using (?:the |a )?(\w+) version"));
    static DEVICES: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)Up to\s+(\d+)\s+devices"));
    static RENEW: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)renewed on\s+(\d{4}-\d{2}-\d{2})"));
    static VALID: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)valid until\s+(\d{4}-\d{2}-\d{2})"));
    static TRAFFIC: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)You have\s+([\d.]+)\s+GB left"));

    let text = strip_ansi(output);
    let low = text.to_lowercase();
    let mut lic = License::default();
    if low.contains("please log in") || low.contains("not logged in") {
        return lic;
    }
    lic.logged_in = true;
    if let Some(c) = EMAIL_RE.captures(&text) {
        lic.email = c[1].to_string();
    }
    if let Some(c) = PLAN.captures(&text) {
        lic.premium = Some(c[1].eq_ignore_ascii_case("premium"));
    }
    if let Some(c) = DEVICES.captures(&text) {
        lic.devices = c[1].parse().ok();
    }
    if let Some(c) = RENEW.captures(&text) {
        lic.renew_date = c[1].to_string();
    }
    if let Some(c) = VALID.captures(&text) {
        lic.valid_until = c[1].to_string();
    }
    if let Some(c) = TRAFFIC.captures(&text) {
        lic.traffic_left_gb = c[1].parse().ok();
    }
    if low.contains("out of free vpn traffic") {
        lic.traffic_exhausted = true;
        lic.traffic_left_gb = Some(0.0);
    }
    lic
}

pub fn parse_email(output: &str) -> String {
    EMAIL_RE.captures(&strip_ansi(output)).map(|c| c[1].to_string()).unwrap_or_default()
}

// --------------------------------------------------------------------------- config

#[derive(Debug, Clone)]
pub struct CliConfig {
    pub mode: String,
    pub socks_port: u16,
    pub socks_host: String,
    pub socks_username: String,
    /// Empty = default (provided by AdGuard VPN).
    pub dns: String,
    /// "", auto, none, script
    pub routing_mode: String,
    pub change_system_dns: Option<bool>,
    pub crash_reporting: Option<bool>,
    pub telemetry: Option<bool>,
    pub update_channel: String,
    pub protocol: String,
    pub post_quantum: bool,
    pub show_hints: bool,
    pub debug_logging: bool,
    pub show_notifications: bool,
    pub bound_if: String,
    pub data_dir: String,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            mode: "tun".into(),
            socks_port: 1080,
            socks_host: "127.0.0.1".into(),
            socks_username: String::new(),
            dns: String::new(),
            routing_mode: String::new(),
            change_system_dns: None,
            crash_reporting: None,
            telemetry: None,
            update_channel: "release".into(),
            protocol: "auto".into(),
            post_quantum: true,
            show_hints: true,
            debug_logging: false,
            show_notifications: false,
            bound_if: String::new(),
            data_dir: String::new(),
        }
    }
}

static PARENS: LazyLock<Regex> = LazyLock::new(|| re(r"\(([^)]*)\)"));

fn default_inner(value: &str) -> String {
    let v = value.trim();
    if v.to_lowercase().starts_with("default") {
        return PARENS.captures(v).map(|c| c[1].trim().to_string()).unwrap_or_default();
    }
    v.to_string()
}

fn parse_bool(value: &str) -> Option<bool> {
    let low = value.trim().to_lowercase();
    if low == "not set" || low.is_empty() {
        return None;
    }
    let low = if low.starts_with("default") { default_inner(&low) } else { low };
    match low.as_str() {
        "on" | "true" | "yes" | "enabled" => Some(true),
        "off" | "false" | "no" | "disabled" => Some(false),
        _ => None,
    }
}

pub fn parse_config(output: &str) -> CliConfig {
    let text = strip_ansi(output);
    let mut cfg = CliConfig::default();
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once(':') else { continue };
        let key = key.trim().to_lowercase();
        let value = value.trim();
        let low = value.to_lowercase();
        match key.as_str() {
            "data directory" => cfg.data_dir = value.into(),
            "mode" => cfg.mode = if value.is_empty() { "tun".into() } else { low },
            "socks port" => {
                if let Ok(port) = value.parse() {
                    cfg.socks_port = port;
                }
            }
            "socks host" => cfg.socks_host = value.into(),
            "socks username" => cfg.socks_username = value.into(),
            "dns upstream" => cfg.dns = if low.starts_with("default") { String::new() } else { value.into() },
            "tunnel routing mode" => {
                cfg.routing_mode = if low == "not set" { String::new() } else { default_inner(value).to_lowercase() }
            }
            "change system dns" => cfg.change_system_dns = parse_bool(value),
            "crash reporting" => cfg.crash_reporting = parse_bool(value),
            "send anonymized usage data" => cfg.telemetry = parse_bool(value),
            "update channel" => {
                let v = default_inner(value).to_lowercase();
                cfg.update_channel = if v.is_empty() { "release".into() } else { v };
            }
            "protocol" => {
                let v = default_inner(value).to_lowercase();
                cfg.protocol = if v.is_empty() { "auto".into() } else { v };
            }
            "post-quantum cryptography" => cfg.post_quantum = parse_bool(value).unwrap_or(false),
            "show hints" => cfg.show_hints = parse_bool(value).unwrap_or(false),
            "debug logging" => cfg.debug_logging = parse_bool(value).unwrap_or(false),
            "show notifications" => cfg.show_notifications = parse_bool(value).unwrap_or(false),
            "outbound interface override" => cfg.bound_if = if low == "not set" { String::new() } else { value.into() },
            _ => {}
        }
    }
    cfg
}

// --------------------------------------------------------------------------- exclusions

/// Returns (mode, items) for `site-exclusions show`.
pub fn parse_exclusions(output: &str) -> (String, Vec<String>) {
    static HEADER: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^Exclusions for (\w+) mode"));
    let text = strip_ansi(output);
    let mut mode = String::new();
    let mut items = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(c) = HEADER.captures(line) {
            mode = c[1].to_lowercase();
            continue;
        }
        let low = line.to_lowercase();
        if low.starts_with("no exclusions") || low.starts_with("you can ") {
            continue;
        }
        items.push(line.to_string());
    }
    (mode, items)
}

pub fn parse_exclusion_mode(output: &str) -> String {
    static MODE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)(?:exclusion mode is|changed to|already set to)\s+(\w+)"));
    MODE.captures(&strip_ansi(output)).map(|c| c[1].to_lowercase()).unwrap_or_else(|| "general".into())
}

// --------------------------------------------------------------------------- misc

pub fn parse_version(output: &str) -> String {
    static VERSION: LazyLock<Regex> = LazyLock::new(|| re(r"v?(\d+\.\d+\.\d+(?:-[\w.]+)?)"));
    VERSION.captures(&strip_ansi(output)).map(|c| c[1].to_string()).unwrap_or_default()
}

pub fn version_tuple(version: &str) -> Vec<u64> {
    static DIGITS: LazyLock<Regex> = LazyLock::new(|| re(r"\d+"));
    let base = version.split('-').next().unwrap_or("");
    DIGITS.find_iter(base).take(3).filter_map(|m| m.as_str().parse().ok()).collect()
}

// Login (runs in a pseudo terminal)
pub static LOGIN_LINK_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(https://\S+)"));
pub const LOGIN_SUCCESS_MARKERS: &[&str] = &["successfully logged in", "you are now logged in", "already logged in"];

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from adguardvpn-cli 1.7.12 `config show`
    const CONFIG: &str = "\x1b[1mCurrent configuration:\n\x1b[0m\tData directory: /home/u/.var/app/x/data/adguardvpn-cli\n\
        \tMode: tun\n\tSOCKS port: 1081\n\tSOCKS host: 127.0.0.1\n\tSOCKS username: bob\n\
        \tDNS upstream: tls://dns.google\n\tTunnel routing mode: not set\n\tChange system DNS: not set\n\
        \tCrash reporting: not set\n\tSend anonymized usage data: not set\n\tUpdate channel: RELEASE\n\
        \tProtocol: Default (auto)\n\tPost-quantum cryptography: Default (on)\n\tShow hints: off\n\
        \tDebug logging: Default (off)\n\tShow notifications: Default (off)\n\tOutbound interface override: not set\n";

    #[test]
    fn config() {
        let cfg = parse_config(CONFIG);
        assert_eq!(cfg.mode, "tun");
        assert_eq!(cfg.socks_port, 1081);
        assert_eq!(cfg.socks_username, "bob");
        assert_eq!(cfg.dns, "tls://dns.google");
        assert_eq!(cfg.routing_mode, "");
        assert_eq!(cfg.change_system_dns, None);
        assert_eq!(cfg.update_channel, "release");
        assert_eq!(cfg.protocol, "auto");
        assert!(cfg.post_quantum);
        assert!(!cfg.show_hints);
        assert!(!cfg.debug_logging);
        assert_eq!(cfg.bound_if, "");
    }

    #[test]
    fn config_default_dns() {
        let cfg = parse_config("\tDNS upstream: Default (provided by AdGuard VPN)\n\tTunnel routing mode: AUTO\n");
        assert_eq!(cfg.dns, "");
        assert_eq!(cfg.routing_mode, "auto");
    }

    #[test]
    fn locations() {
        let text = format!(
            "ISO   COUNTRY              CITY                           PING ESTIMATE\n\
             DE    Germany              Frankfurt                      38        \n\
             US    United States        New York                       118       \n\
             AE    United Arab Emirates Dubai (Virtual)                96        \n\
             {:<5} {:<20} {:<30} {:<10}\n\
             JP    Japan                Tokyo                                    \n",
            "BA", "Bosnia and Herzegovina", "Sarajevo", "72"
        );
        let locs = parse_locations(&text);
        let isos: Vec<_> = locs.iter().map(|l| l.iso.as_str()).collect();
        assert_eq!(isos, ["DE", "US", "AE", "BA", "JP"]);
        assert_eq!(locs[0].city, "Frankfurt");
        assert_eq!(locs[0].ping, Some(38));
        assert_eq!(locs[1].city, "New York");
        assert_eq!(locs[1].country, "United States");
        assert_eq!(locs[2].country, "United Arab Emirates");
        assert_eq!(locs[2].city, "Dubai");
        assert!(locs[2].is_virtual);
        // Country longer than its column
        assert_eq!(locs[3].country, "Bosnia and Herzegovina");
        assert_eq!(locs[3].city, "Sarajevo");
        assert_eq!(locs[3].ping, Some(72));
        assert_eq!(locs[4].ping, None);
    }

    #[test]
    fn status() {
        let st = parse_status("Connected to FRANKFURT in TUN mode, running on tun0\n");
        assert_eq!((st.state, st.location.as_str(), st.mode.as_str(), st.interface.as_str()),
                   (State::Connected, "FRANKFURT", "TUN", "tun0"));
        let st = parse_status("Connected to New York in SOCKS mode, listening on 127.0.0.1:1080\n");
        assert_eq!((st.state, st.location.as_str(), st.listen.as_str()), (State::Connected, "New York", "127.0.0.1:1080"));
        let st = parse_status("Connection lost. Waiting to reconnect to Tokyo in TUN mode, running on tun0");
        assert_eq!((st.state, st.location.as_str()), (State::Reconnecting, "Tokyo"));
        assert_eq!(parse_status("Reconnecting to Tokyo in TUN mode").state, State::Reconnecting);
        assert_eq!(parse_status("VPN is disconnected\n").state, State::Disconnected);
        assert_eq!(parse_status("You are not logged in\nYou can log in by running `x login`").state, State::NotLoggedIn);
        assert_eq!(parse_status("VPN active, but status unavailable").state, State::Connected);
    }

    #[test]
    fn license_premium() {
        let lic = parse_license(
            "Logged in as user@example.com\nYou are using the \x1b[1mPREMIUM\x1b[0m version\n\
             Up to 10 devices simultaneously\nYour subscription is valid until 2027-05-01\n",
        );
        assert!(lic.logged_in);
        assert_eq!(lic.email, "user@example.com");
        assert_eq!(lic.premium, Some(true));
        assert_eq!(lic.devices, Some(10));
        assert_eq!(lic.valid_until, "2027-05-01");
    }

    #[test]
    fn license_free() {
        let lic = parse_license(
            "You are using the FREE version\nYou have 2.47 GB left for this month\n\
             Upgrade at https://agrd.io/vpn_cli to get more locations and higher speeds",
        );
        assert_eq!(lic.premium, Some(false));
        assert!((lic.traffic_left_gb.unwrap() - 2.47).abs() < 1e-9);
        assert!(parse_license("You are out of free VPN traffic this month. VPN speed is limited").traffic_exhausted);
        assert!(!parse_license("Please log in to view your license info").logged_in);
    }

    #[test]
    fn exclusions() {
        let (mode, items) = parse_exclusions("Exclusions for \x1b[1mSELECTIVE\x1b[0m mode:\n*.foo.com\n10.0.0.0/8\n");
        assert_eq!(mode, "selective");
        assert_eq!(items, ["*.foo.com", "10.0.0.0/8"]);
        assert!(parse_exclusions("Exclusions for GENERAL mode:\n\n").1.is_empty());
        assert_eq!(parse_exclusion_mode("Current exclusion mode is \x1b[1mGENERAL\x1b[0m"), "general");
        assert_eq!(parse_exclusion_mode("Exclusion mode changed to SELECTIVE"), "selective");
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("AdGuard VPN CLI v1.7.12\n"), "1.7.12");
        assert_eq!(version_tuple("1.8.2-nightly.2"), [1, 8, 2]);
        assert!(version_tuple("1.10.0") > version_tuple("1.9.9"));
    }
}
