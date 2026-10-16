//! Wrapper around the official adguardvpn-cli binary.
//!
//! All CLI invocations run on the host (via `flatpak-spawn --host` when sandboxed): the VPN
//! service needs the host network namespace, /dev/net/tun and root privileges, none of
//! which exist inside a Flatpak sandbox. Non-privileged commands run as the user; connect,
//! disconnect and the kill switch go through the `agvpn-helper` script under pkexec/sudo.

use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use gtk::glib;

use crate::consts::{CLI_NAME, HOST_HELPER, HOST_HELPER_CLI};
use crate::parsers::{self, CliConfig, License, Location, Status};
use crate::util::{
    args, data_dir, exit_code, find_in_path, host_argv, host_file_exists, host_has, host_path, is_executable, path_str,
    resource_path, run_host, runtime_dir, set_mode, strip_ansi, CmdResult, Env,
};

const fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

pub fn downloaded_cli_dir() -> PathBuf {
    data_dir().join("cli")
}

fn read_version_file(path: &str) -> String {
    std::fs::read_to_string(path).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Locates the CLI binary (path as seen from inside the sandbox).
pub fn find_cli() -> Option<String> {
    if let Ok(env) = std::env::var("AGVPN_CLI") {
        if !env.is_empty() && is_executable(Path::new(&env)) {
            return Some(env);
        }
    }
    // 1. Newer version downloaded by the in-app updater, or the one bundled as extra-data.
    let mut candidates: Vec<(Vec<u64>, String)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(downloaded_cli_dir()) {
        for entry in entries.flatten() {
            let path = entry.path().join(CLI_NAME);
            if is_executable(&path) {
                let version = entry.file_name().to_string_lossy().into_owned();
                candidates.push((parsers::version_tuple(&version), path_str(&path)));
            }
        }
    }
    let bundled = format!("/app/extra/{CLI_NAME}");
    if is_executable(Path::new(&bundled)) {
        let version = read_version_file("/app/extra/VERSION");
        let version = if version.is_empty() { "0.0.0".to_string() } else { version };
        candidates.push((parsers::version_tuple(&version), bundled));
    }
    candidates.sort();
    if let Some((_, path)) = candidates.pop() {
        return Some(path);
    }
    // 2. System-wide installation (running outside Flatpak).
    let mut system: Vec<PathBuf> = find_in_path(CLI_NAME).into_iter().collect();
    system.push(PathBuf::from(format!("/opt/adguardvpn_cli/{CLI_NAME}")));
    system.push(PathBuf::from(format!("/usr/local/bin/{CLI_NAME}")));
    system.into_iter().find(|p| is_executable(p)).map(|p| path_str(&p))
}

pub struct Cli {
    binary: RwLock<Option<String>>,
    version: RwLock<String>,
    pub data_dir: String,
    pub privileged: Privileged,
}

impl Cli {
    pub fn new() -> Arc<Self> {
        let dir = data_dir().join("adguardvpn-cli");
        let _ = std::fs::create_dir_all(&dir);
        let cli = Arc::new(Self {
            binary: RwLock::new(None),
            version: RwLock::new(String::new()),
            data_dir: path_str(&dir),
            privileged: Privileged::default(),
        });
        cli.reload_binary();
        cli
    }

    // ------------------------------------------------------------------ basics

    /// Locates the binary. Cheap; the version is loaded separately (blocking).
    pub fn reload_binary(&self) {
        let binary = find_cli();
        log::info!("CLI: {binary:?}");
        *self.binary.write().unwrap() = binary;
        self.version.write().unwrap().clear();
    }

    pub fn load_version(&self) -> String {
        // The very first run in a fresh data dir fetches backend config and may take ~30 s.
        if self.available() {
            let version = parsers::parse_version(&self.run(&["--version"], secs(90)).out);
            *self.version.write().unwrap() = version;
        }
        self.version()
    }

    pub fn binary(&self) -> Option<String> {
        self.binary.read().unwrap().clone()
    }

    pub fn version(&self) -> String {
        self.version.read().unwrap().clone()
    }

    pub fn available(&self) -> bool {
        self.binary.read().unwrap().is_some()
    }

    pub fn host_binary(&self) -> String {
        host_path(self.binary().as_deref().unwrap_or(CLI_NAME))
    }

    pub fn env(&self) -> Env {
        vec![
            ("AGVPN_CLI_DATA_PATH".into(), self.data_dir.clone()),
            ("TERM".into(), "dumb".into()),
            ("NO_COLOR".into(), "1".into()),
            ("LC_ALL".into(), "C.UTF-8".into()),
        ]
    }

    pub fn run(&self, cli_args: &[&str], timeout: Duration) -> CmdResult {
        if !self.available() {
            return CmdResult::new(127, "adguardvpn-cli not found");
        }
        let mut argv = vec![self.host_binary()];
        argv.extend(cli_args.iter().map(|s| s.to_string()));
        run_host(&argv, &self.env(), timeout, None)
    }

    fn run_owned(&self, cli_args: &[String], timeout: Duration) -> CmdResult {
        let refs: Vec<&str> = cli_args.iter().map(String::as_str).collect();
        self.run(&refs, timeout)
    }

    // ------------------------------------------------------------------ queries

    pub fn status(&self) -> Status {
        parsers::parse_status(&self.run(&["status"], secs(20)).out)
    }

    pub fn license(&self) -> License {
        parsers::parse_license(&self.run(&["license"], secs(40)).out)
    }

    pub fn locations(&self) -> (Vec<Location>, CmdResult) {
        let res = self.run(&["list-locations"], secs(60));
        (parsers::parse_locations(&res.out), res)
    }

    pub fn config(&self) -> CliConfig {
        parsers::parse_config(&self.run(&["config", "show"], secs(15)).out)
    }

    pub fn set_config(&self, key: &str, values: &[String]) -> CmdResult {
        let mut argv = vec!["config".to_string(), key.to_string()];
        argv.extend(values.iter().cloned());
        self.run_owned(&argv, secs(30))
    }

    pub fn exclusions(&self, mode: &str) -> Vec<String> {
        parsers::parse_exclusions(&self.run(&["site-exclusions", "show", "--for-mode", mode], secs(60)).out).1
    }

    pub fn exclusion_mode(&self) -> String {
        parsers::parse_exclusion_mode(&self.run(&["site-exclusions", "mode"], secs(60)).out)
    }

    pub fn set_exclusion_mode(&self, mode: &str) -> CmdResult {
        self.run(&["site-exclusions", "mode", mode], secs(60))
    }

    fn exclusions_cmd(&self, action: &str, items: &[String], mode: &str) -> CmdResult {
        let mut argv = vec!["site-exclusions".to_string(), action.to_string()];
        argv.extend(items.iter().cloned());
        argv.extend(["--for-mode".to_string(), mode.to_string()]);
        self.run_owned(&argv, secs(60))
    }

    pub fn add_exclusions(&self, items: &[String], mode: &str) -> CmdResult {
        self.exclusions_cmd("add", items, mode)
    }

    pub fn remove_exclusions(&self, items: &[String], mode: &str) -> CmdResult {
        self.exclusions_cmd("remove", items, mode)
    }

    pub fn clear_exclusions(&self, mode: &str) -> CmdResult {
        self.run(&["site-exclusions", "clear", "--for-mode", mode], secs(60))
    }

    pub fn logout(&self) -> CmdResult {
        self.run(&["logout"], secs(40))
    }

    pub fn export_logs(&self, path: &str) -> CmdResult {
        self.run(&["export-logs", "-o", path, "-f"], secs(120))
    }

    // ------------------------------------------------------------------ privileged

    pub fn connect(&self, location: Option<&str>, fastest: bool, ip_mode: &str, kill_switch: bool, allow_lan: bool) -> CmdResult {
        let mut tail = Vec::new();
        if fastest {
            tail.push("-f".to_string());
        } else if let Some(location) = location {
            tail.extend(["-l".to_string(), location.to_string()]);
        }
        match ip_mode {
            "ipv4" => tail.push("-4".into()),
            "ipv6" => tail.push("-6".into()),
            _ => {}
        }
        let mut argv = vec!["connect".to_string()];
        if kill_switch {
            argv.push("--kill-switch".into());
            if allow_lan {
                argv.push("--allow-lan".into());
            }
        }
        argv.push("--".into());
        argv.extend(tail);
        // Generous timeout: the user may be typing the password in the polkit dialog.
        self.privileged.run(self, &argv, secs(600))
    }

    pub fn disconnect(&self) -> CmdResult {
        self.privileged.run(self, &args(&["disconnect"]), secs(60))
    }

    pub fn kill_switch_on(&self, allow_lan: bool) -> CmdResult {
        let mut argv = args(&["killswitch-on"]);
        if allow_lan {
            argv.push("--allow-lan".into());
        }
        self.privileged.run(self, &argv, secs(30))
    }

    pub fn kill_switch_off(&self) -> CmdResult {
        self.privileged.run(self, &args(&["killswitch-off"]), secs(30))
    }

    // ------------------------------------------------------------------ login

    /// Starts `adguardvpn-cli login` inside a pseudo terminal (it is interactive).
    pub fn login_session(&self, login_args: &[String]) -> std::io::Result<PtySession> {
        let mut argv = vec![self.host_binary(), "login".to_string()];
        argv.extend(login_args.iter().cloned());
        PtySession::spawn(&argv, &self.env())
    }
}

// =========================================================================== pty

pub enum PtyEvent {
    Output(String),
    Exit(i32),
}

/// A host process attached to a pseudo terminal. Events arrive on `events`.
pub struct PtySession {
    writer: Arc<Mutex<Option<File>>>,
    pid: u32,
    pub events: async_channel::Receiver<PtyEvent>,
}

fn open_pty() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let rc = unsafe {
        libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), std::ptr::null())
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    unsafe { Ok((OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave))) }
}

impl PtySession {
    fn spawn(argv: &[String], env: &Env) -> std::io::Result<Self> {
        let (master, slave) = open_pty()?;
        let (cmd, extra_env) = host_argv(argv, env, true);
        let mut command = Command::new(&cmd[0]);
        command
            .args(&cmd[1..])
            .envs(env.iter().cloned())
            .envs(extra_env)
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        drop(command);
        let pid = child.id();
        let reader_fd = master.try_clone()?;
        let writer = Arc::new(Mutex::new(Some(File::from(master))));
        let (tx, rx) = async_channel::unbounded();
        let writer_for_reader = writer.clone();
        std::thread::spawn(move || {
            let mut reader = File::from(reader_fd);
            let mut buf = [0u8; 4096];
            loop {
                let mut pfd = libc::pollfd { fd: reader.as_raw_fd(), events: libc::POLLIN, revents: 0 };
                let ready = unsafe { libc::poll(&mut pfd, 1, 500) };
                if ready < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    break;
                }
                if ready == 0 {
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        break;
                    }
                    continue;
                }
                match reader.read(&mut buf) {
                    // EIO: the slave side was closed (the process exited).
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let text = strip_ansi(&String::from_utf8_lossy(&buf[..n]));
                        let _ = tx.send_blocking(PtyEvent::Output(text));
                    }
                }
            }
            let code = child.wait().map(|s| exit_code(&s)).unwrap_or(1);
            writer_for_reader.lock().unwrap().take();
            let _ = tx.send_blocking(PtyEvent::Exit(code));
        });
        Ok(Self { writer, pid, events: rx })
    }

    pub fn write(&self, text: &str) {
        if let Some(file) = self.writer.lock().unwrap().as_mut() {
            let _ = file.write_all(text.as_bytes());
        }
    }

    pub fn cancel(&self) {
        self.write("x");
        crate::util::send_signal(self.pid, libc::SIGTERM);
    }
}

// =========================================================================== privileges

/// A request for the user's password: the prompt and where to send the answer
/// (`None` = cancelled).
pub type PasswordRequest = (String, async_channel::Sender<Option<String>>);

/// Runs agvpn-helper as root.
///
/// Order of attempts:
///   1. The helper installed on the host (/usr/local/libexec/agvpn) — a polkit rule and a
///      sudoers entry make it passwordless for the user who installed it.
///   2. pkexec with the bundled helper (graphical polkit password prompt).
///   3. `sudo -A` with an askpass script that asks for the password in our own window —
///      used when no polkit authentication agent is reachable from the host process.
#[derive(Default)]
pub struct Privileged {
    installed: Mutex<Option<bool>>,
    lock: Mutex<()>,
    password_tx: Mutex<Option<async_channel::Sender<PasswordRequest>>>,
}

impl Privileged {
    pub fn set_password_channel(&self, tx: async_channel::Sender<PasswordRequest>) {
        *self.password_tx.lock().unwrap() = Some(tx);
    }

    pub fn bundled_helper(&self) -> String {
        host_path(&path_str(&resource_path("helper/agvpn-helper")))
    }

    pub fn is_installed(&self, refresh: bool) -> bool {
        let mut cached = self.installed.lock().unwrap();
        if cached.is_none() || refresh {
            *cached = Some(host_file_exists(HOST_HELPER) && host_file_exists(HOST_HELPER_CLI));
        }
        cached.unwrap_or(false)
    }

    /// Whether the installed helper's CLI copy is identical to the one the app uses.
    pub fn installed_cli_matches(&self, cli: &Cli) -> bool {
        if !self.is_installed(false) || !cli.available() {
            return false;
        }
        run_host(&args(&["cmp", "-s", HOST_HELPER_CLI, &cli.host_binary()]), &Vec::new(), secs(20), None).ok()
    }

    fn common_args(cli: &Cli) -> Vec<String> {
        vec!["--data".into(), cli.data_dir.clone(), "--cli".into(), cli.host_binary()]
    }

    pub fn run(&self, cli: &Cli, helper_args: &[String], timeout: Duration) -> CmdResult {
        let _guard = self.lock.lock().unwrap();
        if !cli.available() {
            return CmdResult::new(127, "adguardvpn-cli not found");
        }
        let mut full = vec![helper_args[0].clone()];
        full.extend(Self::common_args(cli));
        full.extend(helper_args[1..].iter().cloned());

        let with = |prefix: &[&str], helper: &str| {
            let mut argv = args(prefix);
            argv.push(helper.to_string());
            argv.extend(full.iter().cloned());
            argv
        };

        if self.is_installed(false) {
            // sudoers NOPASSWD entry works everywhere; polkit JS rules need polkit >= 0.106.
            if host_has("sudo") {
                let res = run_host(&with(&["sudo", "-n"], HOST_HELPER), &Vec::new(), timeout, None);
                if !sudo_failed(&res) {
                    return res;
                }
            }
            if host_has("pkexec") {
                let res = run_host(&with(&["pkexec"], HOST_HELPER), &Vec::new(), timeout, None);
                if !pkexec_failed(&res) {
                    return res;
                }
            }
        }

        let helper = self.bundled_helper();
        if host_has("pkexec") {
            let res = run_host(&with(&["pkexec"], &helper), &Vec::new(), timeout, None);
            if !pkexec_failed(&res) {
                return res;
            }
            log::warn!("pkexec failed, falling back to sudo: {}", res.out.trim());
        }
        if host_has("sudo") {
            return self.run_sudo(&helper, &full, timeout);
        }
        CmdResult::new(127, "Neither pkexec nor sudo is available on the host system")
    }

    fn run_sudo(&self, helper: &str, full: &[String], timeout: Duration) -> CmdResult {
        let tx = self.password_tx.lock().unwrap().clone();
        let askpass = match AskPass::new(tx) {
            Ok(askpass) => askpass,
            Err(err) => return CmdResult::new(1, format!("askpass: {err}")),
        };
        let mut argv = args(&["sudo", "-A", "-p", "Password for %p:", helper]);
        argv.extend(full.iter().cloned());
        let env = vec![("SUDO_ASKPASS".to_string(), host_path(&path_str(&askpass.script)))];
        let res = run_host(&argv, &env, timeout, None);
        askpass.close();
        res
    }

    // ------------------------------------------------------------------ helper install

    pub fn install(&self, cli: &Cli) -> CmdResult {
        let res = self.run_install(cli, "install");
        self.is_installed(true);
        res
    }

    pub fn uninstall(&self, cli: &Cli) -> CmdResult {
        let res = self.run_install(cli, "uninstall");
        self.is_installed(true);
        res
    }

    fn run_install(&self, cli: &Cli, command: &str) -> CmdResult {
        let _guard = self.lock.lock().unwrap();
        let helper = self.bundled_helper();
        let mut full = vec![command.to_string()];
        full.extend(Self::common_args(cli));
        if host_has("pkexec") {
            let mut argv = vec!["pkexec".to_string(), helper.clone()];
            argv.extend(full.iter().cloned());
            let res = run_host(&argv, &Vec::new(), secs(120), None);
            if !pkexec_failed(&res) {
                return res;
            }
        }
        self.run_sudo(&helper, &full, secs(120))
    }
}

/// True if pkexec itself could not authorize (as opposed to the helper failing).
fn pkexec_failed(res: &CmdResult) -> bool {
    if res.code != 127 {
        return false;
    }
    let low = res.out.to_lowercase();
    ["error executing command as another user", "not authorized", "authentication agent", "must be setuid root", "cannot run program"]
        .iter()
        .any(|m| low.contains(m))
}

/// True if sudo itself refused (no NOPASSWD entry etc.), not the helper.
fn sudo_failed(res: &CmdResult) -> bool {
    if res.ok() {
        return false;
    }
    let low = res.out.to_lowercase();
    res.code == 1
        && ["password is required", "not allowed to execute", "is not in the sudoers", "may not run sudo", "sudo:"]
            .iter()
            .any(|m| low.contains(m))
}

/// SUDO_ASKPASS bridge: a tiny host-side script talks to our window through FIFOs.
struct AskPass {
    dir: PathBuf,
    request: PathBuf,
    response: PathBuf,
    script: PathBuf,
    stop: Arc<AtomicBool>,
}

fn mkfifo(path: &Path) -> std::io::Result<()> {
    let c = CString::new(path.as_os_str().as_encoded_bytes()).map_err(std::io::Error::other)?;
    if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

impl AskPass {
    fn new(provider: Option<async_channel::Sender<PasswordRequest>>) -> std::io::Result<Self> {
        let dir = runtime_dir().join(format!("askpass-{}-{}", std::process::id(), glib::random_int()));
        std::fs::create_dir(&dir)?;
        set_mode(&dir, 0o700);
        let request = dir.join("request");
        let response = dir.join("response");
        mkfifo(&request)?;
        mkfifo(&response)?;
        let script = dir.join("askpass.sh");
        let (host_request, host_response) = (host_path(&path_str(&request)), host_path(&path_str(&response)));
        std::fs::write(
            &script,
            format!("#!/bin/sh\nprintf '%s\\n' \"${{1:-Password:}}\" > '{host_request}'\ncat '{host_response}'\n"),
        )?;
        set_mode(&script, 0o700);

        let stop = Arc::new(AtomicBool::new(false));
        let (req, resp, stop_flag) = (request.clone(), response.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut cancelled = false;
            while !stop_flag.load(Ordering::SeqCst) {
                let Ok(prompt) = std::fs::read_to_string(&req) else { return };
                if stop_flag.load(Ordering::SeqCst) {
                    return;
                }
                // After a cancel, answer sudo's retries with an empty password right away.
                let password = if cancelled { None } else { ask(provider.as_ref(), prompt.trim()) };
                if password.is_none() {
                    cancelled = true;
                }
                let Ok(mut fh) = std::fs::OpenOptions::new().write(true).open(&resp) else { return };
                let _ = fh.write_all(format!("{}\n", password.unwrap_or_default()).as_bytes());
            }
        });
        Ok(Self { dir, request, response, script, stop })
    }

    fn close(self) {
        self.stop.store(true, Ordering::SeqCst);
        // Unblock the server thread if it still waits on a FIFO.
        use std::os::unix::fs::OpenOptionsExt;
        if let Ok(fh) = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(&self.request) {
            drop(fh);
        }
        if let Ok(mut fh) = std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open(&self.response) {
            let _ = fh.write_all(b"\n");
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn ask(provider: Option<&async_channel::Sender<PasswordRequest>>, prompt: &str) -> Option<String> {
    let provider = provider?;
    let (tx, rx) = async_channel::bounded(1);
    provider.send_blocking((prompt.to_string(), tx)).ok()?;
    rx.recv_blocking().ok().flatten()
}
