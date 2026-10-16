//! Logging to stderr and to a size-rotated file in the cache dir (`agvpn.log`).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use log::{Level, LevelFilter, Log, Metadata, Record};

use crate::util::cache_dir;

const MAX_BYTES: u64 = 1 << 20;
const BACKUPS: u32 = 2;

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
