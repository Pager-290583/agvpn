//! Sandbox-aware process helpers, paths, a serial background worker and simple signals.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::{mpsc, LazyLock};
use std::time::{Duration, Instant};

use gtk::glib;
use regex::Regex;

use crate::consts::APP_ID;

pub static IN_FLATPAK: LazyLock<bool> = LazyLock::new(|| Path::new("/.flatpak-info").exists());

static ANSI_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").unwrap());

pub fn strip_ansi(text: &str) -> String {
    ANSI_RE.replace_all(text, "").replace('\r', "")
}

// --------------------------------------------------------------------------- paths

/// Reads a key from a GKeyFile-style file such as /.flatpak-info.
pub fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut current = "";
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            current = &line[1..line.len() - 1];
        } else if current == section {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

static FLATPAK_APP_PATH: LazyLock<Option<String>> = LazyLock::new(|| {
    if !*IN_FLATPAK {
        return None;
    }
    let text = std::fs::read_to_string("/.flatpak-info").ok()?;
    ini_value(&text, "Instance", "app-path")
});

/// Translates a path visible inside the sandbox to the same file on the host.
pub fn host_path(path: &str) -> String {
    if let Some(app_path) = FLATPAK_APP_PATH.as_deref() {
        if path == "/app" || path.starts_with("/app/") {
            return format!("{}{}", app_path, &path["/app".len()..]);
        }
    }
    path.to_string()
}

fn app_dir(base: PathBuf) -> PathBuf {
    let path = if *IN_FLATPAK { base } else { base.join("agvpn") };
    let _ = std::fs::create_dir_all(&path);
    path
}

/// Per-app data dir. Inside Flatpak it is ~/.var/app/<id>/data, which the host sees too.
pub fn data_dir() -> PathBuf {
    app_dir(glib::user_data_dir())
}

pub fn config_dir() -> PathBuf {
    app_dir(glib::user_config_dir())
}

pub fn cache_dir() -> PathBuf {
    app_dir(glib::user_cache_dir())
}

/// A directory shared between the sandbox and the host (for FIFOs).
pub fn runtime_dir() -> PathBuf {
    let path = if *IN_FLATPAK {
        glib::user_runtime_dir().join("app").join(APP_ID)
    } else {
        glib::user_runtime_dir().join("agvpn")
    };
    let _ = std::fs::create_dir_all(&path);
    set_mode(&path, 0o700);
    path
}

pub fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

/// Location of bundled files (helper script, icons, CSS) for Flatpak, installed and source runs.
pub fn resource_path(rel: &str) -> PathBuf {
    let mut candidates = vec![PathBuf::from("/app/share/agvpn").join(rel)];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin_dir) = exe.parent() {
            // <prefix>/bin/agvpn -> <prefix>/share/agvpn
            candidates.push(bin_dir.join("../share/agvpn").join(rel));
            // <repo>/target/{debug,release}/agvpn -> <repo>
            candidates.push(bin_dir.join("../..").join(rel));
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel));
    for cand in &candidates {
        if cand.exists() {
            return cand.canonicalize().unwrap_or_else(|_| cand.clone());
        }
    }
    candidates.pop().unwrap()
}

pub fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

// --------------------------------------------------------------------------- processes

#[derive(Debug, Clone, Default)]
pub struct CmdResult {
    pub code: i32,
    pub out: String,
}

impl CmdResult {
    pub fn new(code: i32, out: impl Into<String>) -> Self {
        Self { code, out: out.into() }
    }

    pub fn ok(&self) -> bool {
        self.code == 0
    }

    pub fn text(&self) -> String {
        strip_ansi(&self.out)
    }
}

pub type Env = Vec<(String, String)>;

/// Builds argv and extra environment to run a command on the host (outside of the sandbox).
pub fn host_argv(argv: &[String], env: &Env, watch_bus: bool) -> (Vec<String>, Env) {
    if *IN_FLATPAK {
        let mut cmd = vec!["flatpak-spawn".to_string(), "--host".to_string()];
        if watch_bus {
            cmd.push("--watch-bus".into());
        }
        for (key, value) in env {
            cmd.push(format!("--env={key}={value}"));
        }
        cmd.extend(argv.iter().cloned());
        (cmd, Vec::new())
    } else {
        (argv.to_vec(), env.clone())
    }
}

pub fn send_signal(pid: u32, signal: i32) {
    unsafe {
        libc::kill(pid as libc::pid_t, signal);
    }
}

/// Runs a command on the host and returns its exit code and combined stdout/stderr.
pub fn run_host(argv: &[String], env: &Env, timeout: Duration, stdin: Option<&str>) -> CmdResult {
    let (cmd, extra_env) = host_argv(argv, env, false);
    log::debug!("run: {}", cmd.join(" "));
    run_command(&cmd, &extra_env, timeout, stdin)
}

/// Runs a local command with a timeout, merging stdout and stderr.
fn run_command(cmd: &[String], extra_env: &Env, timeout: Duration, stdin: Option<&str>) -> CmdResult {
    let (reader, writer) = match std::io::pipe() {
        Ok(pair) => pair,
        Err(err) => return CmdResult::new(127, err.to_string()),
    };
    let writer_err = match writer.try_clone() {
        Ok(w) => w,
        Err(err) => return CmdResult::new(127, err.to_string()),
    };
    let mut command = Command::new(&cmd[0]);
    command
        .args(&cmd[1..])
        .envs(extra_env.iter().cloned())
        .stdout(writer)
        .stderr(writer_err)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return CmdResult::new(127, err.to_string()),
    };
    // The pipe's write ends live in `command`; drop them so EOF can arrive.
    drop(command);
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let text = text.to_string();
        std::thread::spawn(move || {
            let _ = pipe.write_all(text.as_bytes());
        });
    }
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    let mut timed_out = false;
    let status = loop {
        while let Ok(chunk) = rx.try_recv() {
            out.extend(chunk);
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if Instant::now() >= deadline {
            timed_out = true;
            // SIGKILL would only kill flatpak-spawn and leave the host process running;
            // SIGTERM is forwarded to it.
            send_signal(child.id(), libc::SIGTERM);
            let grace = Instant::now() + Duration::from_secs(10);
            let status = loop {
                if let Ok(Some(status)) = child.try_wait() {
                    break Some(status);
                }
                if Instant::now() >= grace {
                    let _ = child.kill();
                    break child.wait().ok();
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            break status;
        }
        let _ = rx.recv_timeout(Duration::from_millis(30)).map(|chunk| out.extend(chunk));
    };
    // Collect the rest of the output. A daemon started by the command may keep the pipe
    // open, so do not wait for EOF forever.
    let drain_until = Instant::now() + Duration::from_secs(2);
    loop {
        let left = drain_until.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(chunk) => out.extend(chunk),
            Err(_) => break,
        }
    }
    let mut text = String::from_utf8_lossy(&out).into_owned();
    if timed_out {
        text.push_str("\nTimed out");
        return CmdResult::new(124, text);
    }
    let code = status.map(|s| exit_code(&s)).unwrap_or(1);
    log::debug!("exit {code}: {}", text.trim().chars().take(500).collect::<String>());
    CmdResult::new(code, text)
}

pub fn exit_code(status: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

pub fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(program)).find(|p| is_executable(p))
}

pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

pub fn find_in_path(program: &str) -> Option<PathBuf> {
    which(program)
}

pub fn host_has(program: &str) -> bool {
    if *IN_FLATPAK {
        let script = format!("command -v \"{program}\" >/dev/null 2>&1");
        return run_host(&args(&["sh", "-c", &script]), &Vec::new(), Duration::from_secs(10), None).ok();
    }
    which(program).is_some()
}

pub fn host_file_exists(path: &str) -> bool {
    if *IN_FLATPAK {
        return run_host(&args(&["test", "-e", path]), &Vec::new(), Duration::from_secs(10), None).ok();
    }
    Path::new(path).exists()
}

// --------------------------------------------------------------------------- threading

type Job = Box<dyn FnOnce() + Send>;

/// Runs blocking jobs one at a time on a background thread; results arrive on the main loop.
#[derive(Clone)]
pub struct Worker {
    tx: mpsc::Sender<Job>,
}

impl Worker {
    pub fn new(name: &str) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                for job in rx {
                    job();
                }
            })
            .expect("cannot start the worker thread");
        Self { tx }
    }

    pub fn submit<T, F, C>(&self, job: F, done: C)
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
        C: FnOnce(T) + 'static,
    {
        let (tx, rx) = async_channel::bounded::<T>(1);
        let wrapped: Job = Box::new(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
            match result {
                Ok(value) => {
                    let _ = tx.send_blocking(value);
                }
                Err(_) => log::error!("background job panicked"),
            }
        });
        if self.tx.send(wrapped).is_err() {
            log::error!("worker thread is gone");
            return;
        }
        glib::spawn_future_local(async move {
            if let Ok(value) = rx.recv().await {
                done(value);
            }
        });
    }
}

/// Runs a one-off blocking job without queueing behind other CLI calls.
pub fn run_in_thread<T, F, C>(job: F, done: C)
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
    C: FnOnce(T) + 'static,
{
    let (tx, rx) = async_channel::bounded::<T>(1);
    std::thread::spawn(move || {
        let value = job();
        let _ = tx.send_blocking(value);
    });
    glib::spawn_future_local(async move {
        if let Ok(value) = rx.recv().await {
            done(value);
        }
    });
}

// --------------------------------------------------------------------------- signals

/// A list of main-thread callbacks; emitting is reentrancy-safe.
pub struct Signal<T> {
    handlers: RefCell<Vec<Rc<dyn Fn(&T)>>>,
}

impl<T> Default for Signal<T> {
    fn default() -> Self {
        Self { handlers: RefCell::new(Vec::new()) }
    }
}

impl<T> Signal<T> {
    pub fn connect(&self, f: impl Fn(&T) + 'static) {
        self.handlers.borrow_mut().push(Rc::new(f));
    }

    pub fn emit(&self, value: T) {
        let handlers: Vec<_> = self.handlers.borrow().clone();
        for handler in handlers {
            handler(&value);
        }
    }
}

// --------------------------------------------------------------------------- formatting

pub fn human_bytes(value: f64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut value = value;
    let mut idx = 0;
    while value >= 1024.0 && idx < units.len() - 1 {
        value /= 1024.0;
        idx += 1;
    }
    if idx == 0 { format!("{value:.0} {}", units[idx]) } else { format!("{value:.1} {}", units[idx]) }
}

pub fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn read_os_release() -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Ok(text) = std::fs::read_to_string("/etc/os-release") {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.to_string(), v.trim_matches('"').to_string());
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_and_bytes() {
        assert_eq!(strip_ansi("\x1b[1mBold\x1b[0m\r\n"), "Bold\n");
        assert_eq!(human_bytes(512.0), "512 B");
        assert_eq!(human_bytes(1536.0), "1.5 KB");
    }

    #[test]
    fn ini() {
        let text = "[Application]\nname=x\n[Instance]\napp-path=/var/lib/flatpak/app/x\n";
        assert_eq!(ini_value(text, "Instance", "app-path").as_deref(), Some("/var/lib/flatpak/app/x"));
        assert_eq!(ini_value(text, "Instance", "name"), None);
    }

    #[test]
    fn run_and_timeout() {
        let res = run_command(&args(&["sh", "-c", "echo out; echo err >&2; exit 3"]), &Vec::new(),
                              Duration::from_secs(5), None);
        assert_eq!(res.code, 3);
        assert!(res.out.contains("out") && res.out.contains("err"));
        let res = run_command(&args(&["sleep", "5"]), &Vec::new(), Duration::from_millis(200), None);
        assert_eq!(res.code, 124);
    }
}
