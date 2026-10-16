//! Local VPN usage statistics: data, connection time and most used locations.
//!
//! Traffic is measured from the TUN interface counters in /sys/class/net, stored per hour
//! in `stats.json` (same format as the Python version of the app).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::util::{data_dir, now_secs};

pub const PERIODS: &[(&str, &str)] = &[("24h", "24 hours"), ("7d", "7 days"), ("30d", "30 days"), ("all", "All time")];

fn period_secs(period: &str) -> Option<i64> {
    match period {
        "24h" => Some(24 * 3600),
        "7d" => Some(7 * 86400),
        "30d" => Some(30 * 86400),
        _ => None,
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct Bucket {
    #[serde(default)]
    rx: f64,
    #[serde(default)]
    tx: f64,
    #[serde(default)]
    secs: f64,
    #[serde(default)]
    loc: BTreeMap<String, f64>,
}

#[derive(Serialize, Deserialize, Default)]
struct Data {
    #[serde(default)]
    since: f64,
    #[serde(default)]
    hours: BTreeMap<String, Bucket>,
}

pub struct Summary {
    pub rx: f64,
    pub tx: f64,
    pub secs: f64,
    /// Location name and seconds, most used first.
    pub locations: Vec<(String, f64)>,
    pub since: f64,
}

pub fn read_iface_counters(iface: &str) -> Option<(u64, u64)> {
    let base = format!("/sys/class/net/{iface}/statistics");
    let rx = std::fs::read_to_string(format!("{base}/rx_bytes")).ok()?.trim().parse().ok()?;
    let tx = std::fs::read_to_string(format!("{base}/tx_bytes")).ok()?.trim().parse().ok()?;
    Some((rx, tx))
}

pub struct Stats {
    path: PathBuf,
    data: Data,
    last_counters: Option<(String, u64, u64)>,
    last_tick: Option<f64>,
    dirty: u32,
}

impl Stats {
    pub fn new() -> Self {
        let mut stats = Self {
            path: data_dir().join("stats.json"),
            data: Data::default(),
            last_counters: None,
            last_tick: None,
            dirty: 0,
        };
        stats.load();
        stats
    }

    fn load(&mut self) {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => match serde_json::from_str::<Data>(&text) {
                Ok(data) => self.data = data,
                Err(err) => log::error!("cannot read stats: {err}"),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => log::error!("cannot read stats: {err}"),
        }
    }

    fn save(&mut self) {
        let tmp = self.path.with_extension("json.tmp");
        let text = serde_json::to_string(&self.data).unwrap_or_default();
        if let Err(err) = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &self.path)) {
            log::error!("cannot save stats: {err}");
        }
        self.dirty = 0;
    }

    pub fn clear(&mut self) {
        self.data = Data::default();
        self.last_counters = None;
        self.last_tick = None;
        self.save();
    }

    /// Called on every status poll.
    pub fn tick(&mut self, connected: bool, iface: &str, location: &str) {
        let now = now_secs();
        if !connected {
            self.last_counters = None;
            self.last_tick = None;
            if self.dirty > 0 {
                self.save();
            }
            return;
        }
        if self.data.since == 0.0 {
            self.data.since = now.floor();
        }
        let key = ((now / 3600.0) as i64).to_string();
        let bucket = self.data.hours.entry(key).or_default();
        if let Some(last) = self.last_tick {
            let elapsed = (now - last).clamp(0.0, 120.0);
            bucket.secs += elapsed;
            if !location.is_empty() {
                *bucket.loc.entry(location.to_string()).or_default() += elapsed;
            }
        }
        self.last_tick = Some(now);
        if let Some((rx, tx)) = if iface.is_empty() { None } else { read_iface_counters(iface) } {
            if let Some((last_iface, last_rx, last_tx)) = &self.last_counters {
                if last_iface == iface && rx >= *last_rx && tx >= *last_tx {
                    bucket.rx += (rx - last_rx) as f64;
                    bucket.tx += (tx - last_tx) as f64;
                }
            }
            self.last_counters = Some((iface.to_string(), rx, tx));
        }
        self.dirty += 1;
        if self.dirty >= 10 {
            self.save();
        }
    }

    pub fn flush(&mut self) {
        if self.dirty > 0 {
            self.save();
        }
    }

    pub fn summary(&self, period: &str) -> Summary {
        let now_hour = (now_secs() / 3600.0) as i64;
        let min_hour = period_secs(period).map(|span| now_hour - span / 3600 + 1);
        let (mut rx, mut tx, mut secs) = (0.0, 0.0, 0.0);
        let mut locs: BTreeMap<String, f64> = BTreeMap::new();
        for (key, bucket) in &self.data.hours {
            let hour: i64 = key.parse().unwrap_or(0);
            if min_hour.is_some_and(|min| hour < min) {
                continue;
            }
            rx += bucket.rx;
            tx += bucket.tx;
            secs += bucket.secs;
            for (loc, value) in &bucket.loc {
                *locs.entry(loc.clone()).or_default() += value;
            }
        }
        let mut locations: Vec<(String, f64)> = locs.into_iter().collect();
        locations.sort_by(|a, b| b.1.total_cmp(&a.1));
        Summary { rx, tx, secs, locations, since: self.data.since }
    }
}
