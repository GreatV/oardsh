use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, Url, WebviewWindow};
use tauri_plugin_notification::NotificationExt;

use crate::i18n;
use crate::paths;

const LOG_LIMIT: usize = 400;
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// The dsh page can call `native_web_event`, so its body is untrusted length.
const NOTIFICATION_BODY_LIMIT: usize = 180;
const HOST: &str = "127.0.0.1";

/// The main window's original URL, captured at setup so stopping can navigate
/// back. It differs per platform and build, so it must not be hardcoded.
static BOOT_URL: OnceLock<Url> = OnceLock::new();

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
}

struct Inner {
    phase: Phase,
    url: Option<String>,
    error: Option<String>,
    child: Option<Child>,
    logs: VecDeque<LogLine>,
    /// Bumped on every start and stop. A launch thread carries the generation
    /// it was spawned for and stands down once it no longer matches.
    generation: u64,
}

impl Inner {
    fn snapshot(&self) -> Status {
        Status {
            phase: self.phase,
            url: self.url.clone(),
            error: self.error.clone(),
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
                child: None,
                logs: VecDeque::new(),
                generation: 0,
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

    pub fn start(&self, app: &AppHandle) {
        let generation = {
            let mut inner = self.lock();
            if matches!(inner.phase, Phase::Starting | Phase::Ready) {
                return;
            }
            inner.generation += 1;
            inner.phase = Phase::Starting;
            inner.url = None;
            inner.error = None;
            inner.logs.clear();
            inner.generation
        };
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
            inner.child = Some(child);
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
                if http_ready(&host, *port) {
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
        }
        emit_status(app, self);
        show_dsh(app, &url)
    }

    fn fail(&self, app: &AppHandle, generation: u64, message: &str) {
        let child = {
            let mut inner = self.lock();
            if inner.generation != generation {
                return;
            }
            inner.phase = Phase::Error;
            inner.error = Some(message.to_string());
            inner.child.take()
        };
        if let Some(mut child) = child {
            kill_child(&mut child);
            let _ = child.wait();
        }
        emit_status(app, self);
    }

    fn try_reap(&self) -> Option<i32> {
        let mut inner = self.lock();
        let child = inner.child.as_mut()?;
        match child.try_wait() {
            Ok(Some(status)) => {
                inner.child = None;
                Some(status.code().unwrap_or(-1))
            }
            _ => None,
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
        let (generation, child) = {
            let mut inner = self.lock();
            if inner.phase == Phase::Idle && inner.child.is_none() {
                return;
            }
            inner.generation += 1;
            inner.phase = Phase::Stopping;
            inner.url = None;
            (inner.generation, inner.child.take())
        };
        emit_status(app, self);
        if let Some(mut child) = child {
            kill_child(&mut child);
            let _ = child.wait();
        }
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeWebEvent {
    kind: String,
    body: String,
    language: String,
}

#[tauri::command]
pub fn native_web_event(app: AppHandle, event: NativeWebEvent) {
    let title_key = match event.kind.as_str() {
        "approval" => "notification.approval.title",
        "question" => "notification.question.title",
        "completed" => "notification.completed.title",
        _ => "notification.updated.title",
    };
    let body_key = match event.kind.as_str() {
        "approval" => "notification.approval.body",
        "question" => "notification.question.body",
        "completed" => "notification.webCompleted.body",
        _ => "notification.updated.body",
    };
    let title = i18n::translate(&event.language, title_key);
    let body = if event.body.trim().is_empty() {
        i18n::translate(&event.language, body_key)
    } else {
        truncate(event.body.trim(), NOTIFICATION_BODY_LIMIT)
    };
    let _ = app
        .notification()
        .builder()
        .title(&title)
        .body(&body)
        .show();
}

/// Cut on a character count, never a byte offset, so multi-byte input cannot panic.
fn truncate(value: &str, limit: usize) -> String {
    match value.char_indices().nth(limit) {
        Some((end, _)) => format!("{}…", &value[..end]),
        None => value.to_string(),
    }
}

pub fn restart_from_menu(app: &AppHandle) {
    let engine = app.state::<Engine>();
    engine.stop(app, true);
    engine.start(app);
}

pub fn boot_dsh(app: &AppHandle) {
    app.state::<Engine>().start(app);
}

fn emit_status(app: &AppHandle, engine: &Engine) {
    let status = engine.status();
    let _ = app.emit(
        "dsh-status",
        StatusEvent {
            phase: status.phase,
            url: status.url,
            error: status.error,
        },
    );
}

/// Point the main window at the running server and bring it forward.
fn show_dsh(app: &AppHandle, url: &str) -> Result<(), String> {
    let parsed = Url::parse(url).map_err(|err| err.to_string())?;
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window is missing".to_string())?;
    window.navigate(parsed).map_err(|err| err.to_string())?;
    let _ = window.unminimize();
    window.show().map_err(|err| err.to_string())?;
    window.set_focus().map_err(|err| err.to_string())
}

pub fn reload_main(app: &AppHandle) {
    if let Some(window) = current_window(app) {
        let _ = window.eval("location.reload()");
    }
}

pub fn current_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
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

fn http_ready(host: &str, port: u16) -> bool {
    let address = format!("{host}:{port}");
    let Ok(mut addrs) = address.to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(250)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(300)));
    let request = format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 24];
    matches!(stream.read(&mut buf), Ok(n) if n >= 5 && buf.starts_with(b"HTTP/"))
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
        command.arg(program).args(args);
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
        .env("CI", "1")
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
    use super::{announced_url, truncate};

    #[test]
    fn truncates_on_character_boundaries() {
        assert_eq!(truncate("short", 8), "short");
        assert_eq!(truncate("abcdefghij", 4), "abcd…");
        // Byte slicing here would panic: each character is three bytes.
        assert_eq!(truncate("需要你的批准", 3), "需要你…");
    }

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
