//! Persistent app-side settings (everything the CLI itself does not store).
//!
//! Stored as `settings.json` in the same format as the Python version of the app, so an
//! existing configuration keeps working.

use std::cell::RefCell;
use std::path::PathBuf;

use serde_json::{json, Map, Value};

use crate::util::{config_dir, Signal};

fn defaults() -> Map<String, Value> {
    let value = json!({
        "onboarding_done": false,
        "agreements_accepted": false,
        // App
        "autostart": false,
        "show_window_on_start": true,
        "auto_connect": false,
        "notifications": true,
        "theme": "system",      // system | light | dark
        "language": "system",   // system | en | ru
        "hide_tray_icon": false,
        "close_to_tray": true,
        // VPN
        "kill_switch": false,
        "kill_switch_allow_lan": true,
        "kill_switch_active": false, // boot id while rules are installed on the host
        "ip_mode": "auto",           // auto | ipv4 | ipv6
        "last_location": "",         // connect_arg of the last used location ("" = fastest)
        "saved_locations": [],       // Location::key values
        "locations_tab": "all",
        // DNS
        "dns_selected": "default",
        "custom_dns": [],            // [{"name": ..., "address": ...}]
        // Exclusions: service id -> list of patterns, per mode
        "services": {"general": {}, "selective": {}},
        // Stats
        "stats_enabled": true,
        // Updates
        "auto_update_cli": true,
    });
    match value {
        Value::Object(map) => map,
        _ => unreachable!(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CustomDns {
    pub name: String,
    pub address: String,
}

pub struct Settings {
    path: PathBuf,
    data: RefCell<Map<String, Value>>,
    /// Emitted with the changed key, or "*" after a reset.
    pub changed: Signal<String>,
}

impl Settings {
    pub fn new() -> Self {
        let settings = Self {
            path: config_dir().join("settings.json"),
            data: RefCell::new(defaults()),
            changed: Signal::default(),
        };
        settings.load();
        settings
    }

    fn load(&self) {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(stored)) => self.data.borrow_mut().extend(stored),
                Ok(_) => {}
                Err(err) => log::error!("cannot read {}: {err}", self.path.display()),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => log::error!("cannot read {}: {err}", self.path.display()),
        }
    }

    fn save(&self) {
        let tmp = self.path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&*self.data.borrow()).unwrap_or_default();
        if let Err(err) = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &self.path)) {
            log::error!("cannot save settings: {err}");
        }
    }

    pub fn get(&self, key: &str) -> Value {
        self.data
            .borrow()
            .get(key)
            .cloned()
            .or_else(|| defaults().remove(key))
            .unwrap_or(Value::Null)
    }

    pub fn bool(&self, key: &str) -> bool {
        match self.get(key) {
            Value::Bool(b) => b,
            Value::Null => false,
            Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
            Value::String(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
        }
    }

    pub fn string(&self, key: &str) -> String {
        match self.get(key) {
            Value::String(s) => s,
            _ => String::new(),
        }
    }

    pub fn strings(&self, key: &str) -> Vec<String> {
        match self.get(key) {
            Value::Array(items) => items.into_iter().filter_map(|v| v.as_str().map(String::from)).collect(),
            _ => Vec::new(),
        }
    }

    pub fn set(&self, key: &str, value: impl Into<Value>) {
        let value = value.into();
        if self.data.borrow().get(key) == Some(&value) {
            return;
        }
        self.data.borrow_mut().insert(key.to_string(), value);
        self.save();
        self.changed.emit(key.to_string());
    }

    // ------------------------------------------------------------------ typed helpers

    pub fn custom_dns(&self) -> Vec<CustomDns> {
        match self.get("custom_dns") {
            Value::Array(items) => items
                .iter()
                .filter_map(|item| {
                    let address = item.get("address")?.as_str()?.to_string();
                    let name = item.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                    Some(CustomDns { name, address })
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn set_custom_dns(&self, items: &[CustomDns]) {
        let value: Vec<Value> = items.iter().map(|i| json!({"name": i.name, "address": i.address})).collect();
        self.set("custom_dns", value);
    }

    /// Service id -> patterns that were added for it, for the given exclusion mode.
    pub fn services(&self, mode: &str) -> Vec<(String, Vec<String>)> {
        let all = self.get("services");
        let Some(Value::Object(map)) = all.get(mode) else { return Vec::new() };
        map.iter()
            .map(|(id, patterns)| {
                let list = patterns
                    .as_array()
                    .map(|a| a.iter().filter_map(|p| p.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                (id.clone(), list)
            })
            .collect()
    }

    pub fn set_services(&self, mode: &str, services: &[(String, Vec<String>)]) {
        let mut all = match self.get("services") {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        let map: Map<String, Value> = services.iter().map(|(id, p)| (id.clone(), json!(p))).collect();
        all.insert(mode.to_string(), Value::Object(map));
        self.set("services", Value::Object(all));
    }

    // ------------------------------------------------------------------ import / export

    pub fn export(&self) -> Value {
        let mut data = self.data.borrow().clone();
        data.remove("kill_switch_active");
        Value::Object(data)
    }

    pub fn import(&self, data: &Map<String, Value>) {
        let known = defaults();
        {
            let mut current = self.data.borrow_mut();
            for (key, value) in data {
                if known.contains_key(key) && key != "kill_switch_active" {
                    current.insert(key.clone(), value.clone());
                }
            }
        }
        self.save();
        for key in data.keys() {
            self.changed.emit(key.clone());
        }
    }

    pub fn reset(&self) {
        let keep = ["onboarding_done", "agreements_accepted", "kill_switch_active", "saved_locations", "custom_dns", "services"];
        let mut fresh = defaults();
        {
            let current = self.data.borrow();
            for key in keep {
                if let Some(value) = current.get(key) {
                    fresh.insert(key.to_string(), value.clone());
                }
            }
        }
        *self.data.borrow_mut() = fresh;
        self.save();
        self.changed.emit("*".into());
    }
}
