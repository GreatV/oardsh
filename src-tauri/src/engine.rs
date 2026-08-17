use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, Url, WebviewWindow};

use crate::i18n;
use crate::paths;
use crate::proxy;
use crate::ready;
use crate::sidecar;

const LOG_LIMIT: usize = 400;
const START_TIMEOUT: Duration = Duration::from_secs(120);
const HOST: &str = "127.0.0.1";
const WATCH_INTERVAL: Duration = Duration::from_millis(400);
const AUTO_RESTART_MAX: u32 = 1;
const STABLE_AFTER: Duration = Duration::from_secs(30);
/// Official `dsh web` default. A listener here that is not our sidecar is a
/// second writer on ~/.dsh.
const PEER_PORT: u16 = 3080;

/// The main window's original URL, captured at setup so stopping can navigate
/// back. It differs per platform and build, so it must not be hardcoded.
static BOOT_URL: OnceLock<Url> = OnceLock::new();

enum Supervised {
    Spawned(Child),
    Adopted { pid: u32, entry: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Idle,
    Starting,
    Ready,
    Stopping,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub stream: String,
    pub line: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub phase: Phase,
    pub url: Option<String>,
    pub error: Option<String>,
    pub crashed: bool,
    pub logs: Vec<LogLine>,
}

/// `Status` minus the logs: `app.emit` reaches every webview, and once dsh is
/// serving, the main window is a remote dsh page. The boot screen reads logs
/// from `dsh_status`, which only the local origin may call.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusEvent {
    phase: Phase,
    url: Option<String>,
    error: Option<String>,
    crashed: bool,
}

struct Inner {
    phase: Phase,
    url: Option<String>,
    error: Option<String>,
    crashed: bool,
    supervised: Option<Supervised>,
    logs: VecDeque<LogLine>,
    /// Bumped on every start and stop. A launch thread carries the generation
    /// it was spawned for and stands down once it no longer matches.
    generation: u64,
    auto_restarts: u32,
    ready_at: Option<Instant>,
    raise_on_ready: bool,
    peer_dsh: bool,
}

impl Inner {
    fn snapshot(&self) -> Status {
        Status {
            phase: self.phase,
            url: self.url.clone(),
            error: self.error.clone(),
            crashed: self.crashed,
            logs: self.logs.iter().cloned().collect(),
        }
    }
}

/// Supervises the single bundled dsh web server. dsh owns the workspace concept
/// in its own page, so the shell never reasons about project directories.
pub struct Engine {
    inner: Mutex<Inner>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                phase: Phase::Idle,
                url: None,
                error: None,
                crashed: false,
                supervised: None,
                logs: VecDeque::new(),
                generation: 0,
                auto_restarts: 0,
                ready_at: None,
                raise_on_ready: true,
                peer_dsh: false,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub fn status(&self) -> Status {
        self.lock().snapshot()
    }

    fn generation(&self) -> u64 {
        self.lock().generation
    }

    pub fn suppress_raise(&self) {
        self.lock().raise_on_ready = false;
    }

    pub fn start(&self, app: &AppHandle) {
        self.start_with(app, true, true, None);
    }

    fn start_with(
        &self,
        app: &AppHandle,
        raise: bool,
        reset_backoff: bool,
        expected_generation: Option<u64>,
    ) {
        let generation = {
            let mut inner = self.lock();
            if let Some(expected) = expected_generation {
                if inner.generation != expected {
                    return;
                }
            }
            if matches!(inner.phase, Phase::Starting | Phase::Ready) {
                return;
            }
            inner.generation += 1;
            inner.phase = Phase::Starting;
            inner.url = None;
            inner.error = None;
            inner.crashed = false;
            inner.ready_at = None;
            inner.raise_on_ready = raise;
            inner.peer_dsh = false;
            if reset_backoff {
                inner.auto_restarts = 0;
            }
            inner.logs.clear();
            inner.generation
        };
        set_tray_tooltip(app, "tray.tooltip.starting");
        emit_status(app, self);

        let handle = app.clone();
        thread::spawn(move || {
            let engine = handle.state::<Engine>();
            if let Err(message) = engine.start_process(&handle, generation) {
                engine.fail(&handle, generation, &message);
            }
        });
    }

    fn start_process(&self, app: &AppHandle, generation: u64) -> Result<(), String> {
        paths::ensure_desktop_plugin(app)?;
        let program = paths::resolve_dsh(app)?;
        if let Some((pid, entry, url)) = sidecar::recover_ours(
            |candidate| {
                Url::parse(candidate)
                    .ok()
                    .and_then(|parsed| parsed.port_or_known_default())
                    .is_some_and(|port| ready::dsh_serving(HOST, port))
            },
            &proxy::fingerprint(app, &proxy::load()),
        ) {
            {
                let mut inner = self.lock();
                if inner.generation != generation {
                    return Ok(());
                }
                inner.supervised = Some(Supervised::Adopted { pid, entry });
            }
            self.push_log(
                "stdout",
                format!("Adopting leftover dsh pid {pid} at {url}"),
            );
            return self.mark_ready(app, generation, url);
        }
        let patch = paths::desktop_patch(app)?;
        let host = HOST.to_string();
        let args = vec![
            "web".to_string(),
            "--patch".to_string(),
            patch.display().to_string(),
            "--host".to_string(),
            host.clone(),
            "--port".to_string(),
            "0".to_string(),
        ];
        let display = format!(
            "{} web --patch {} --host {host} --port 0",
            program.display(),
            patch.display()
        );
        self.push_log("stdout", format!("Launching {display}"));
        if ready::dsh_serving(HOST, PEER_PORT) {
            {
                let mut inner = self.lock();
                if inner.generation == generation {
                    inner.peer_dsh = true;
                }
            }
            self.push_log(
                "stdout",
                "Another dsh is already serving on 127.0.0.1:3080; oardsh will use its own port. Running both can corrupt shared sessions.".into(),
            );
        }

        // dsh resolves working directories from its own registry; the server
        // just needs somewhere stable to run.
        let cwd = paths::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let mut command = dsh_command(app, &program, &args)?;
        configure_command(app, &mut command, &cwd);
        let mut child = command
            .spawn()
            .map_err(|err| format!("Failed to start dsh: {err}"))?;
        self.push_log("stdout", format!("pid {}", child.id()));

        let (url_tx, url_rx) = mpsc::channel();
        if let Some(stdout) = child.stdout.take() {
            spawn_reader(app.clone(), "stdout", stdout, Some(url_tx));
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_reader(app.clone(), "stderr", stderr, None);
        }
        {
            let mut inner = self.lock();
            if inner.generation != generation {
                kill_child(&mut child);
                return Ok(());
            }
            sidecar::write(
                child.id(),
                &program,
                None,
                Some(&proxy::fingerprint(app, &proxy::load())),
            );
            inner.supervised = Some(Supervised::Spawned(child));
        }

        let started = Instant::now();
        let mut discovered: Option<(String, u16)> = None;
        while started.elapsed() < START_TIMEOUT {
            if self.generation() != generation {
                return Ok(());
            }
            if let Ok(url) = url_rx.try_recv() {
                if let Ok(parsed) = Url::parse(&url) {
                    if let Some(port) = parsed.port_or_known_default() {
                        discovered = Some((url, port));
                    }
                }
            }
            if let Some((url, port)) = discovered.as_ref() {
                if ready::dsh_serving(&host, *port) {
                    return self.mark_ready(app, generation, url.clone());
                }
            }
            if let Some(code) = self.try_reap() {
                return Err(format!(
                    "dsh exited before the UI was ready (code {code}).\n{}",
                    self.recent_log_text()
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err("Timed out waiting for dsh to announce its dynamic port".into())
    }

    fn mark_ready(&self, app: &AppHandle, generation: u64, url: String) -> Result<(), String> {
        {
            let mut inner = self.lock();
            if inner.generation != generation {
                return Ok(());
            }
            inner.phase = Phase::Ready;
            inner.url = Some(url.clone());
            inner.error = None;
            inner.crashed = false;
            inner.ready_at = Some(Instant::now());
        }
        sidecar::update_url(&url);
        set_tray_tooltip(app, "tray.tooltip.ready");
        emit_status(app, self);
        let raise = self.lock().raise_on_ready;
        present_dsh(app, &url, raise)?;
        self.watch_ready(app, generation);
        Ok(())
    }

    fn watch_ready(&self, app: &AppHandle, generation: u64) {
        let handle = app.clone();
        thread::spawn(move || loop {
            thread::sleep(WATCH_INTERVAL);
            let engine = handle.state::<Engine>();
            if engine.generation() != generation {
                return;
            }
            if !matches!(engine.status().phase, Phase::Ready) {
                return;
            }
            {
                let mut inner = engine.lock();
                if inner.auto_restarts > 0 {
                    if let Some(ready_at) = inner.ready_at {
                        if ready_at.elapsed() >= STABLE_AFTER {
                            inner.auto_restarts = 0;
                        }
                    }
                }
            }
            if let Some(code) = engine.try_reap() {
                engine.crash(&handle, generation, code);
                return;
            }
        });
    }

    fn crash(&self, app: &AppHandle, generation: u64, code: i32) {
        let window = current_window(app);
        let visible = window
            .as_ref()
            .and_then(|window| window.is_visible().ok())
            .unwrap_or(false);
        let focused = window
            .as_ref()
            .and_then(|window| window.is_focused().ok())
            .unwrap_or(false);
        let (auto, gen_at_crash) = {
            let mut inner = self.lock();
            if inner.generation != generation {
                return;
            }
            inner.supervised = None;
            inner.url = None;
            inner.ready_at = None;
            if inner.auto_restarts < AUTO_RESTART_MAX {
                inner.auto_restarts += 1;
                inner.phase = Phase::Idle;
                inner.crashed = false;
                inner.error = None;
                (true, inner.generation)
            } else {
                inner.phase = Phase::Error;
                inner.crashed = true;
                let recent = inner
                    .logs
                    .iter()
                    .rev()
                    .take(24)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .map(|line| format!("[{}] {}", line.stream, line.line))
                    .collect::<Vec<_>>()
                    .join("\n");
                inner.error = Some(format!("dsh exited while serving (code {code}).\n{recent}"));
                (false, inner.generation)
            }
        };
        sidecar::clear();
        if auto {
            if visible {
                if let (Some(window), Some(url)) = (app.get_webview_window("main"), BOOT_URL.get())
                {
                    let _ = window.navigate(url.clone());
                }
            }
            self.start_with(app, focused, false, Some(gen_at_crash));
            self.push_log(
                "stdout",
                format!("Auto-restarting after unexpected exit (code {code})"),
            );
            return;
        }
        set_tray_tooltip(app, "tray.tooltip.stopped");
        emit_status(app, self);
        if let (Some(window), Some(url)) = (app.get_webview_window("main"), BOOT_URL.get()) {
            let _ = window.navigate(url.clone());
        }
        reveal_main(app);
    }

    fn fail(&self, app: &AppHandle, generation: u64, message: &str) {
        let supervised = {
            let mut inner = self.lock();
            if inner.generation != generation {
                return;
            }
            inner.phase = Phase::Error;
            inner.crashed = false;
            inner.error = Some(message.to_string());
            inner.supervised.take()
        };
        if let Some(mut supervised) = supervised {
            kill_supervised(&mut supervised);
        }
        sidecar::clear();
        set_tray_tooltip(app, "tray.tooltip.stopped");
        emit_status(app, self);
        // Always restore the boot page so a later tray reveal shows the
        // failure UI, not a dead dsh URL. Do not raise a hidden window.
        if let (Some(window), Some(url)) = (app.get_webview_window("main"), BOOT_URL.get()) {
            let _ = window.navigate(url.clone());
        }
    }

    fn try_reap(&self) -> Option<i32> {
        let mut inner = self.lock();
        match inner.supervised.as_mut()? {
            Supervised::Spawned(child) => match child.try_wait() {
                Ok(Some(status)) => {
                    inner.supervised = None;
                    Some(status.code().unwrap_or(-1))
                }
                _ => None,
            },
            Supervised::Adopted { pid, .. } => {
                if sidecar::pid_alive(*pid) {
                    None
                } else {
                    inner.supervised = None;
                    Some(-1)
                }
            }
        }
    }

    fn recent_log_text(&self) -> String {
        let inner = self.lock();
        inner
            .logs
            .iter()
            .rev()
            .take(24)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|line| format!("[{}] {}", line.stream, line.line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Buffer a line for the failure report. Never broadcast: dsh's stdout can
    /// carry workspace paths and prompt fragments. See `StatusEvent`.
    pub fn push_log(&self, stream: &str, line: String) {
        if line.trim().is_empty() {
            return;
        }
        let entry = LogLine {
            stream: stream.to_string(),
            line,
        };
        let mut inner = self.lock();
        inner.logs.push_back(entry);
        while inner.logs.len() > LOG_LIMIT {
            inner.logs.pop_front();
        }
    }

    /// Stop the server. `release_window` returns the window to the boot screen;
    /// a window that is already closing must not be navigated.
    pub fn stop(&self, app: &AppHandle, release_window: bool) {
        let (generation, supervised) = {
            let mut inner = self.lock();
            // Always bump: crash() drops to Idle before start_with, and a
            // quit in that window must invalidate the pending restart.
            inner.generation += 1;
            if inner.phase == Phase::Idle && inner.supervised.is_none() {
                return;
            }
            inner.phase = Phase::Stopping;
            inner.url = None;
            inner.crashed = false;
            (inner.generation, inner.supervised.take())
        };
        emit_status(app, self);
        if let Some(mut supervised) = supervised {
            kill_supervised(&mut supervised);
        }
        sidecar::clear();
        set_tray_tooltip(app, "tray.tooltip.stopped");
        {
            // A start() that raced in while the child was dying owns the phase
            // now; writing Idle would strand it as never started.
            let mut inner = self.lock();
            if inner.generation != generation {
                return;
            }
            inner.phase = Phase::Idle;
        }
        if release_window {
            if let (Some(window), Some(url)) = (app.get_webview_window("main"), BOOT_URL.get()) {
                let _ = window.navigate(url.clone());
            }
        }
        emit_status(app, self);
    }
}

/// Capture the main window's real URL before anything navigates it away.
pub fn remember_boot_url(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if let Ok(url) = window.url() {
            let _ = BOOT_URL.set(url);
        }
    }
}

#[tauri::command]
pub fn dsh_status(engine: State<Engine>) -> Status {
    engine.status()
}

#[tauri::command]
pub fn restart_dsh(app: AppHandle) {
    restart_from_menu(&app);
}

pub fn restart_from_menu(app: &AppHandle) {
    let engine = app.state::<Engine>();
    let raise = current_window(app)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);
    engine.stop(app, true);
    engine.start_with(app, raise, true, None);
}

pub fn boot_dsh(app: &AppHandle) {
    app.state::<Engine>().start(app);
}

pub fn quit_app(app: &AppHandle) {
    app.state::<Engine>().stop(app, false);
    app.exit(0);
}

pub fn reveal_main(app: &AppHandle) {
    if let Some(window) = current_window(app) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn emit_status(app: &AppHandle, engine: &Engine) {
    let status = engine.status();
    let _ = app.emit(
        "dsh-status",
        StatusEvent {
            phase: status.phase,
            url: status.url,
            error: status.error,
            crashed: status.crashed,
        },
    );
}

/// Point the main window at the running server. `raise` is false when an
/// auto-restart happens while the user left the window in the tray.
fn present_dsh(app: &AppHandle, url: &str, raise: bool) -> Result<(), String> {
    let parsed = Url::parse(url).map_err(|err| err.to_string())?;
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window is missing".to_string())?;
    window.navigate(parsed).map_err(|err| err.to_string())?;
    if raise {
        let _ = window.unminimize();
        window.show().map_err(|err| err.to_string())?;
        window.set_focus().map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn set_tray_tooltip(app: &AppHandle, key: &str) {
    let text = i18n::translate(i18n::system_locale(), key);
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(text));
    }
}

pub fn note_hidden_to_tray(app: &AppHandle) {
    app.state::<Engine>().suppress_raise();
}

pub fn reload_main(app: &AppHandle) {
    if let Some(window) = current_window(app) {
        let _ = window.eval("location.reload()");
    }
}

pub fn current_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}

fn kill_supervised(supervised: &mut Supervised) {
    match supervised {
        Supervised::Spawned(child) => {
            kill_child(child);
            let _ = child.wait();
        }
        Supervised::Adopted { pid, entry } => sidecar::kill_if_ours(*pid, entry),
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    app: AppHandle,
    stream: &'static str,
    reader: R,
    url_tx: Option<Sender<String>>,
) {
    thread::spawn(move || {
        let engine = app.state::<Engine>();
        let buf = BufReader::new(reader);
        for line in buf.lines() {
            match line {
                Ok(text) => {
                    if let Some(url) = announced_url(&text) {
                        if let Some(tx) = url_tx.as_ref() {
                            let _ = tx.send(url);
                        }
                    }
                    engine.push_log(stream, text);
                }
                Err(_) => break,
            }
        }
    });
}

/// Recover the dynamic port dsh bound to. The banner wording is not a contract,
/// so any loopback URL is accepted as a fallback rather than stalling the
/// launch for START_TIMEOUT on a reworded line.
fn announced_url(line: &str) -> Option<String> {
    const MARKER: &str = "dsh web: ";
    if let Some(start) = line.find(MARKER) {
        if let Some(url) = loopback_url(line[start + MARKER.len()..].trim_start()) {
            return Some(url);
        }
    }
    let start = line.find("http://")?;
    loopback_url(&line[start..])
}

fn loopback_url(value: &str) -> Option<String> {
    if !value.starts_with("http://") {
        return None;
    }
    let end = value
        .find(|character: char| {
            character.is_whitespace() || matches!(character, '"' | '\'' | '<' | '>' | ')' | ',')
        })
        .unwrap_or(value.len());
    let candidate = value[..end].trim_end_matches(['.', ';', ':']);
    let parsed = Url::parse(candidate).ok()?;
    // An explicit port keeps a docs link from being mistaken for the server;
    // a loopback host keeps us probing only this machine.
    parsed.port()?;
    match parsed.host_str()? {
        "127.0.0.1" | "localhost" | "0.0.0.0" | "[::1]" | "::1" => Some(candidate.to_string()),
        _ => None,
    }
}

pub(crate) fn dsh_command(
    app: &AppHandle,
    program: &Path,
    args: &[String],
) -> Result<Command, String> {
    // No explicit `cmd /C` for batch files: Command runs .cmd/.bat itself with
    // the CVE-2024-24576 escaping, and routing args through our own cmd would
    // opt back out of it.
    if is_node_script(program) {
        let node = paths::resolve_node(app)?;
        let mut command = Command::new(node);
        // canonicalize() on Windows prefixes `\\?\`; Node 22+ then lstats `C:`.
        command
            .arg(paths::strip_windows_namespace(program.to_path_buf()))
            .args(args);
        return Ok(command);
    }
    let mut command = Command::new(program);
    command.args(args);
    Ok(command)
}

pub(crate) fn configure_command(app: &AppHandle, command: &mut Command, workspace: &Path) {
    command
        .current_dir(workspace)
        .env("PATH", paths::path_with_package_bins(app))
        .env(
            "HOME",
            paths::home_dir().unwrap_or_else(|| workspace.to_path_buf()),
        )
        .env("CI", "1");
    if let Some(home) = paths::resolve_dsh_home() {
        command.env("DSH_HOME", home);
    }
    proxy::apply(command);
    command
        .env("NPM_CONFIG_YES", "true")
        .env("NPM_CONFIG_UPDATE_NOTIFIER", "false")
        .env("NPM_CONFIG_PROGRESS", "false")
        .env("npm_config_fund", "false")
        .env("npm_config_audit", "false")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_process_isolation(command);
}

fn is_node_script(program: &Path) -> bool {
    if program
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("js"))
    {
        return true;
    }
    let Ok(mut file) = File::open(program) else {
        return false;
    };
    let mut buf = [0u8; 96];
    let Ok(n) = file.read(&mut buf) else {
        return false;
    };
    let head = String::from_utf8_lossy(&buf[..n]);
    head.contains("#!/usr/bin/env node") || head.contains("#!/usr/bin/node")
}

fn apply_process_isolation(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
}

pub(crate) fn kill_child(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        unsafe {
            libc::killpg(pid, libc::SIGTERM);
        }
        thread::sleep(Duration::from_millis(250));
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .status();
    }
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::announced_url;

    #[test]
    fn parses_dynamic_port_announcement() {
        assert_eq!(
            announced_url("dsh web: http://127.0.0.1:54732"),
            Some("http://127.0.0.1:54732".into())
        );
        assert_eq!(announced_url("unrelated output"), None);
    }

    #[test]
    fn falls_back_to_any_loopback_url_when_the_banner_changes() {
        assert_eq!(
            announced_url("  ➜  Local:  http://localhost:41234/  "),
            Some("http://localhost:41234/".into())
        );
        assert_eq!(
            announced_url("listening on http://127.0.0.1:8080."),
            Some("http://127.0.0.1:8080".into())
        );
    }

    #[test]
    fn ignores_urls_that_cannot_be_the_local_server() {
        // No explicit port: a docs link, not a dynamically bound server.
        assert_eq!(announced_url("see http://localhost/guide"), None);
        // Not loopback.
        assert_eq!(announced_url("dsh web: http://example.com:8080"), None);
    }
}
