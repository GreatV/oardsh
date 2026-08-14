use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, Url, WebviewWindow};

use crate::paths;
use crate::settings::{self, Settings};

const LOG_LIMIT: usize = 400;

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
    pub workspace: Option<String>,
    pub port: u16,
    pub host: String,
    pub command: Option<String>,
    pub error: Option<String>,
    pub attached: bool,
    pub logs: Vec<LogLine>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartOptions {
    pub workspace: String,
    pub port: u16,
    pub host: Option<String>,
    pub dsh_path: Option<String>,
    pub auto_launch: Option<bool>,
}

struct Inner {
    phase: Phase,
    url: Option<String>,
    workspace: Option<String>,
    port: u16,
    host: String,
    command: Option<String>,
    error: Option<String>,
    attached: bool,
    child: Option<Child>,
    logs: VecDeque<LogLine>,
}

impl Inner {
    fn snapshot(&self) -> Status {
        Status {
            phase: self.phase,
            url: self.url.clone(),
            workspace: self.workspace.clone(),
            port: self.port,
            host: self.host.clone(),
            command: self.command.clone(),
            error: self.error.clone(),
            attached: self.attached,
            logs: self.logs.iter().cloned().collect(),
        }
    }
}

pub struct Engine {
    inner: Mutex<Inner>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                phase: Phase::Idle,
                url: None,
                workspace: None,
                port: 3080,
                host: "127.0.0.1".into(),
                command: None,
                error: None,
                attached: false,
                child: None,
                logs: VecDeque::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|err| err.into_inner())
    }

    pub fn status(&self) -> Status {
        self.lock().snapshot()
    }

    pub fn stop(&self, app: &AppHandle) {
        let child = {
            let mut inner = self.lock();
            inner.phase = Phase::Stopping;
            inner.attached = false;
            emit_status(app, &inner.snapshot());
            inner.child.take()
        };
        if let Some(mut child) = child {
            kill_child(&mut child);
            let _ = child.wait();
        }
        let mut inner = self.lock();
        inner.phase = Phase::Idle;
        inner.url = None;
        inner.command = None;
        emit_status(app, &inner.snapshot());
    }

    pub fn start(&self, app: &AppHandle, options: StartOptions) -> Result<Status, String> {
        let workspace = PathBuf::from(options.workspace.trim());
        if !workspace.is_dir() {
            return Err(format!(
                "Workspace is not a directory: {}",
                workspace.display()
            ));
        }
        if options.port == 0 {
            return Err("Port must be between 1 and 65535".into());
        }
        let host = options
            .host
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("127.0.0.1")
            .to_string();
        if host == "0.0.0.0" {
            return Err("dsh web does not support --host 0.0.0.0 yet".into());
        }

        {
            let inner = self.lock();
            if inner.phase == Phase::Starting {
                return Err("dsh is already starting".into());
            }
            if inner.phase == Phase::Ready
                && inner.workspace.as_deref() == Some(&workspace.display().to_string())
                && inner.port == options.port
                && inner.host == host
            {
                if let Some(url) = inner.url.clone() {
                    let _ = navigate_main(app, &url);
                }
                return Ok(inner.snapshot());
            }
        }

        self.stop(app);

        let (program, mut args, display_base) =
            paths::resolve_launch(app, options.dsh_path.as_deref())?;
        args.push("--host".into());
        args.push(host.clone());
        args.push("--port".into());
        args.push(options.port.to_string());

        let url = format!("http://{host}:{}", options.port);
        let display = format!("{display_base} --host {host} --port {}", options.port);

        persist_start_settings(app, &workspace, options.port, &host, &options);

        {
            let mut inner = self.lock();
            inner.phase = Phase::Starting;
            inner.error = None;
            inner.attached = false;
            inner.workspace = Some(workspace.display().to_string());
            inner.port = options.port;
            inner.host = host.clone();
            inner.command = Some(display.clone());
            inner.url = Some(url.clone());
            inner.logs.clear();
            emit_status(app, &inner.snapshot());
        }

        self.push_log(app, "stdout", format!("Launching {display}"));
        self.push_log(
            app,
            "stdout",
            format!("Workspace {}", workspace.display()),
        );

        if http_ready(&host, options.port) {
            self.push_log(
                app,
                "stdout",
                format!("Attached to an existing server at {url}"),
            );
            let status = self.mark_ready(app, true);
            let _ = navigate_main(app, &url);
            return Ok(status);
        }

        let mut command = build_command(app, &program, &args)?;
        command
            .current_dir(&workspace)
            .env("PATH", paths::path_with_package_bins(app))
            .env("HOME", paths::home_dir().unwrap_or_else(|| workspace.clone()))
            .env("CI", "1")
            .env("NPM_CONFIG_YES", "true")
            .env("npm_config_yes", "true")
            .env("NPM_CONFIG_UPDATE_NOTIFIER", "false")
            .env("npm_config_update_notifier", "false")
            .env("NPM_CONFIG_PROGRESS", "false")
            .env("npm_config_fund", "false")
            .env("npm_config_audit", "false")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        apply_process_isolation(&mut command);

        let mut child = command.spawn().map_err(|err| {
            let message = format!("Failed to start dsh: {err}");
            self.fail(app, &message);
            message
        })?;

        self.push_log(app, "stdout", format!("pid {}", child.id()));

        if let Some(stdout) = child.stdout.take() {
            spawn_reader(app.clone(), "stdout", stdout);
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_reader(app.clone(), "stderr", stderr);
        }

        {
            let mut inner = self.lock();
            inner.child = Some(child);
        }

        let started = Instant::now();
        let mut last_heartbeat = Instant::now();
        loop {
            if http_ready(&host, options.port) {
                let status = self.mark_ready(app, false);
                let _ = navigate_main(app, &url);
                return Ok(status);
            }

            if let Some(code) = self.try_reap() {
                let logs = self.recent_log_text();
                let message = format!(
                    "dsh exited before the UI was ready (code {code}).\n{logs}"
                );
                self.fail(app, &message);
                return Err(message);
            }

            if last_heartbeat.elapsed() >= Duration::from_secs(5) {
                self.push_log(
                    app,
                    "stdout",
                    format!(
                        "Waiting for {url} ({}s). First boot initializes the local dsh web profile.",
                        started.elapsed().as_secs()
                    ),
                );
                last_heartbeat = Instant::now();
            }

            thread::sleep(Duration::from_millis(150));
        }
    }

    fn mark_ready(&self, app: &AppHandle, attached: bool) -> Status {
        let mut inner = self.lock();
        inner.phase = Phase::Ready;
        inner.attached = attached;
        inner.error = None;
        let status = inner.snapshot();
        emit_status(app, &status);
        status
    }

    fn fail(&self, app: &AppHandle, message: &str) {
        let child = {
            let mut inner = self.lock();
            inner.phase = Phase::Error;
            inner.error = Some(message.to_string());
            inner.attached = false;
            emit_status(app, &inner.snapshot());
            inner.child.take()
        };
        if let Some(mut child) = child {
            kill_child(&mut child);
            let _ = child.wait();
        }
        if !is_boot_url(app) {
            let _ = show_boot(app);
        }
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
        self.lock()
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

    pub fn push_log(&self, app: &AppHandle, stream: &str, line: String) {
        if line.trim().is_empty() {
            return;
        }
        let entry = LogLine {
            stream: stream.to_string(),
            line,
        };
        let _ = app.emit("dsh-log", &entry);
        let mut inner = self.lock();
        inner.logs.push_back(entry);
        while inner.logs.len() > LOG_LIMIT {
            inner.logs.pop_front();
        }
    }
}

#[tauri::command]
pub fn probe_environment(app: AppHandle) -> paths::Environment {
    paths::probe(&app)
}

#[tauri::command]
pub fn get_status(engine: State<Engine>) -> Status {
    engine.status()
}

#[tauri::command]
pub fn retry_dsh(app: AppHandle) {
    boot_dsh(&app);
}

pub fn show_boot(app: &AppHandle) -> Result<(), String> {
    navigate_main(app, &boot_url())
}

pub fn pick_workspace(app: &AppHandle) -> Option<PathBuf> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .set_title("Choose a DeepSeek Harness workspace")
        .blocking_pick_folder()
        .and_then(|path| path.into_path().ok())
}

pub fn launch_from_menu(app: &AppHandle, workspace: PathBuf) {
    let mut stored = settings::load(app);
    settings::remember_workspace(&mut stored, &workspace.display().to_string());
    let _ = settings::save(app, &stored);
    let _ = app.emit("settings-changed", &stored);
    let _ = show_boot(app);
    let options = StartOptions {
        workspace: workspace.display().to_string(),
        port: stored.port,
        host: Some(stored.host),
        dsh_path: stored.dsh_path,
        auto_launch: Some(true),
    };
    let handle = app.clone();
    thread::spawn(move || {
        let engine = handle.state::<Engine>();
        let _ = engine.start(&handle, options);
    });
}

pub fn restart_from_menu(app: &AppHandle) {
    let _ = show_boot(app);
    boot_dsh(app);
}

pub fn boot_dsh(app: &AppHandle) {
    let stored = settings::load(app);
    let workspace = resolve_workspace(&stored);
    let handle = app.clone();
    thread::spawn(move || {
        let engine = handle.state::<Engine>();
        let _ = engine.start(
            &handle,
            StartOptions {
                workspace,
                port: stored.port,
                host: Some(stored.host),
                dsh_path: stored.dsh_path,
                auto_launch: Some(true),
            },
        );
    });
}

fn resolve_workspace(stored: &Settings) -> String {
    if let Some(workspace) = stored.workspace.as_deref() {
        if Path::new(workspace).is_dir() {
            return workspace.to_string();
        }
    }
    paths::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .display()
        .to_string()
}

fn persist_start_settings(
    app: &AppHandle,
    workspace: &Path,
    port: u16,
    host: &str,
    options: &StartOptions,
) {
    let mut stored = settings::load(app);
    settings::remember_workspace(&mut stored, &workspace.display().to_string());
    stored.port = port;
    stored.host = host.to_string();
    if options.dsh_path.is_some() {
        stored.dsh_path = options.dsh_path.clone();
    }
    if let Some(auto_launch) = options.auto_launch {
        stored.auto_launch = auto_launch;
    }
    let _ = settings::save(app, &stored);
    let _ = app.emit("settings-changed", &stored);
}

fn emit_status(app: &AppHandle, status: &Status) {
    let _ = app.emit("dsh-status", status);
}

pub fn navigate_main(app: &AppHandle, url: &str) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is missing".to_string())?;
    let parsed = Url::parse(url).map_err(|err| err.to_string())?;
    window.navigate(parsed).map_err(|err| err.to_string())?;
    let _ = window.set_title("DeepSeek Harness");
    Ok(())
}

pub fn boot_url() -> String {
    if cfg!(dev) {
        "http://localhost:1420/".into()
    } else if cfg!(target_os = "windows") {
        "http://tauri.localhost/".into()
    } else {
        "tauri://localhost/".into()
    }
}

fn is_boot_url(app: &AppHandle) -> bool {
    let Some(window) = app.get_webview_window("main") else {
        return false;
    };
    let Ok(url) = window.url() else {
        return false;
    };
    let value = url.as_str();
    value.contains(":1420") || value.starts_with("tauri://") || value.contains("tauri.localhost")
}

fn spawn_reader<R: Read + Send + 'static>(app: AppHandle, stream: &'static str, reader: R) {
    thread::spawn(move || {
        let engine = app.state::<Engine>();
        let buf = BufReader::new(reader);
        for line in buf.lines() {
            match line {
                Ok(text) => engine.push_log(&app, stream, text),
                Err(_) => break,
            }
        }
    });
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

fn build_command(app: &AppHandle, program: &Path, args: &[String]) -> Result<Command, String> {
    #[cfg(windows)]
    {
        let ext = program
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "cmd" || ext == "bat" {
            let mut command = Command::new("cmd");
            command.arg("/C").arg(program).args(args);
            return Ok(command);
        }
    }
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

fn is_node_script(program: &Path) -> bool {
    let is_js = program
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("js"));
    if is_js {
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

fn kill_child(child: &mut Child) {
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
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .status();
    }
    let _ = child.kill();
}

pub fn reload_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.reload();
    }
}

#[allow(dead_code)]
pub fn current_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}
