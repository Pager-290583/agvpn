//! Logging to stderr and to a size-rotated file in the cache dir (`agvpn.log`).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::LazyLock;

use log::{Level, LevelFilter, Log, Metadata, Record};

use crate::util::cache_dir;

const MAX_BYTES: u64 = 1 << 20;
const BACKUPS: u32 = 2;

/// Old debug entries may contain command arguments and multi-line CLI output.
/// Omit the entire entry, including continuation lines, from diagnostic exports.
pub fn diagnostic_log(text: &str) -> String {
    static ENTRY: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} (ERROR|WARN|INFO|DEBUG|TRACE) ").unwrap()
    });
    let mut include = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        if let Some(caps) = ENTRY.captures(line) {
            include = !matches!(&caps[1], "DEBUG" | "TRACE");
        }
        if include {
            lines.push(line);
        }
    }
    lines.join("\n")
}

struct Logger {
    level: LevelFilter,
    path: PathBuf,
    file: Mutex<Option<File>>,
}

impl Logger {
    fn open(&self) -> Option<File> {
        OpenOptions::new().create(true).append(true).open(&self.path).ok()
    }

    fn rotate(&self) {
        for i in (1..BACKUPS).rev() {
            let _ = std::fs::rename(self.path.with_extension(format!("log.{i}")), self.path.with_extension(format!("log.{}", i + 1)));
        }
        let _ = std::fs::rename(&self.path, self.path.with_extension("log.1"));
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let now = gtk::glib::DateTime::now_local()
            .ok()
            .and_then(|dt| dt.format("%Y-%m-%d %H:%M:%S").ok())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let line = format!("{now} {} {}: {}\n", record.level(), record.target(), record.args());
        eprint!("{line}");
        let mut guard = self.file.lock().unwrap();
        if guard.as_ref().and_then(|f| f.metadata().ok()).is_some_and(|m| m.len() > MAX_BYTES) {
            *guard = None;
            self.rotate();
        }
        if guard.is_none() {
            *guard = self.open();
        }
        if let Some(file) = guard.as_mut() {
            let _ = file.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Some(file) = self.file.lock().unwrap().as_mut() {
            let _ = file.flush();
        }
    }
}

pub fn init() {
    let level = if std::env::var_os("AGVPN_DEBUG").is_some() { LevelFilter::Debug } else { LevelFilter::Info };
    let logger = Logger { level, path: cache_dir().join("agvpn.log"), file: Mutex::new(None) };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(level);
    }
    let _ = Level::Info;
}

#[cfg(test)]
mod tests {
    #[test]
    fn diagnostics_omit_old_debug_entries_and_continuations() {
        let text = "orphaned secret\n2026-11-02 12:00:00 INFO agvpn: ready\n\
            2026-11-02 12:00:01 DEBUG agvpn.cli: run: config set-socks-password fake-secret\n\
            continuation-secret\n2026-11-02 12:00:02 WARN agvpn: network lost\n";
        let clean = super::diagnostic_log(text);
        assert!(clean.contains("ready") && clean.contains("network lost"));
        assert!(!clean.contains("secret") && !clean.contains("DEBUG"));
    }
}
