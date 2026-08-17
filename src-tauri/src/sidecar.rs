use std::fs;
use std::path::{Path, PathBuf};
#[cfg(any(windows, target_os = "macos"))]
use std::process::Command;
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::paths;

/// Written after we spawn dsh, so a later oardsh launch can adopt our leftover
/// or reap it. A pid alone is not a licence to kill: the record names the
/// entry script, and we only touch a live process whose command line still
/// contains that path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    pid: u32,
    entry: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    proxy: Option<String>,
}

pub fn write(pid: u32, entry: &Path, url: Option<&str>, proxy: Option<&str>) {
    let Some(path) = sidecar_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let record = Record {
        pid,
        entry: entry.display().to_string(),
        url: url.map(str::to_string),
        proxy: proxy.map(str::to_string),
    };
    if let Ok(body) = serde_json::to_string(&record) {
        let _ = persist_atomic(&path, body.as_bytes());
    }
}

pub fn update_url(url: &str) {
    let Some(path) = sidecar_path() else {
        return;
    };
    recover_backup(&path);
    let Ok(body) = fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut record) = serde_json::from_str::<Record>(&body) else {
        return;
    };
    record.url = Some(url.to_string());
    if let Ok(next) = serde_json::to_string(&record) {
        let _ = persist_atomic(&path, next.as_bytes());
    }
}

pub fn clear() {
    if let Some(path) = sidecar_path() {
        let _ = fs::remove_file(path);
    }
}

fn tray_hint_path() -> Option<PathBuf> {
    paths::resolve_dsh_home().map(|home| PathBuf::from(home).join("oardsh.tray-hint"))
}

pub fn tray_hint_seen() -> bool {
    tray_hint_path().is_some_and(|path| path.is_file())
}

pub fn mark_tray_hint_seen() {
    let Some(path) = tray_hint_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, "1\n");
}

/// If a leftover process is ours and still serving, return it so the engine
/// can attach. If it is ours but not serving, kill only that process. A live
/// process whose command line is not our entry is left alone.
pub fn recover_ours(
    ready: impl Fn(&str) -> bool,
    expected_proxy: &str,
) -> Option<(u32, PathBuf, String)> {
    let path = sidecar_path()?;
    recover_backup(&path);
    let body = fs::read_to_string(&path).ok()?;
    let record = serde_json::from_str::<Record>(&body).ok()?;
    let entry = PathBuf::from(&record.entry);
    if record.proxy.as_deref() != Some(expected_proxy) {
        kill_if_ours(record.pid, &entry);
        let _ = fs::remove_file(&path);
        return None;
    }
    if !pid_alive(record.pid) {
        let _ = fs::remove_file(path);
        return None;
    }
    if !owns_process(record.pid, &entry) {
        // Pid reuse: the record now points at someone else. Drop the file only.
        let _ = fs::remove_file(path);
        return None;
    }
    if let Some(url) = record.url.as_deref() {
        if is_loopback_server(url) && ready(url) {
            return Some((record.pid, entry, url.to_string()));
        }
    }
    kill_if_ours(record.pid, &entry);
    let _ = fs::remove_file(path);
    None
}

pub fn kill_if_ours(pid: u32, entry: &Path) {
    if !owns_process(pid, entry) {
        return;
    }
    #[cfg(unix)]
    {
        unsafe {
            libc::killpg(pid as i32, libc::SIGTERM);
        }
        thread::sleep(Duration::from_millis(250));
        unsafe {
            libc::killpg(pid as i32, libc::SIGKILL);
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .status();
    }
}

pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let Ok(output) = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .creation_flags(0x0800_0000)
            .output()
        else {
            return false;
        };
        if !output.status.success() {
            return false;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        !text.to_ascii_lowercase().contains("no tasks") && text.contains(&format!("\"{pid}\""))
    }
}

fn owns_process(pid: u32, entry: &Path) -> bool {
    let Some(cmdline) = process_command_line(pid) else {
        return false;
    };
    cmdline_owns_entry(&cmdline, &entry.display().to_string())
}

fn cmdline_owns_entry(cmdline: &str, entry: &str) -> bool {
    !entry.is_empty() && cmdline.contains(entry)
}

fn process_command_line(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let bytes = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        Some(String::from_utf8_lossy(&bytes).replace('\0', " "))
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("ps")
            .args(["-p", &pid.to_string(), "-ww", "-o", "command="])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-CimInstance Win32_Process -Filter \"ProcessId={pid}\").CommandLine"),
            ])
            .creation_flags(0x0800_0000)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = pid;
        None
    }
}

fn sidecar_path() -> Option<PathBuf> {
    paths::resolve_dsh_home().map(|home| PathBuf::from(home).join("oardsh.sidecar.json"))
}

pub(crate) fn recover_backup(path: &Path) {
    if path.is_file() {
        return;
    }
    let bak = path.with_extension("bak");
    if bak.is_file() {
        let _ = fs::rename(&bak, path);
    }
}

/// Write via a sibling temp file, then rename. `fs::write` truncates first, so
/// a crash mid-update would leave an unreadable record and a leaked dsh.
pub(crate) fn persist_atomic(path: &Path, body: &[u8]) -> std::io::Result<()> {
    recover_backup(path);
    let tmp = path.with_extension("tmp");
    write_private(&tmp, body)?;
    if fs::rename(&tmp, path).is_ok() {
        return Ok(());
    }
    // Windows cannot rename over an existing file. Move the old file aside
    // first so a failed second rename can put it back.
    let bak = path.with_extension("bak");
    let _ = fs::remove_file(&bak);
    fs::rename(path, &bak)?;
    match fs::rename(&tmp, path) {
        Ok(()) => {
            let _ = fs::remove_file(&bak);
            Ok(())
        }
        Err(err) => {
            let _ = fs::rename(&bak, path);
            let _ = fs::remove_file(&tmp);
            Err(err)
        }
    }
}

fn write_private(path: &Path, body: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(body)?;
        file.sync_all()?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, body)
    }
}

fn is_loopback_server(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("http://") else {
        return false;
    };
    let hostport = rest.split('/').next().unwrap_or(rest);
    let Some((host, port)) = hostport.rsplit_once(':') else {
        return false;
    };
    !port.is_empty()
        && port.chars().all(|c| c.is_ascii_digit())
        && matches!(
            host,
            "127.0.0.1" | "localhost" | "[::1]" | "::1" | "0.0.0.0"
        )
}

#[cfg(test)]
mod tests {
    use super::{cmdline_owns_entry, is_loopback_server, persist_atomic, recover_backup, Record};
    use std::fs;

    #[test]
    fn identity_requires_the_entry_path() {
        let entry =
            "/Users/me/oardsh.app/Contents/Resources/dsh/node_modules/@deepseek-ai/dsh/lib/bin.js";
        let ours =
            format!("node {entry} web --patch /tmp/oardsh.patch.yml --host 127.0.0.1 --port 0");
        assert!(cmdline_owns_entry(&ours, entry));
        // A user-started CLI dsh lives in the npm cache, not our bundle.
        assert!(!cmdline_owns_entry(
            "node /Users/me/.npm/_npx/hash/node_modules/@deepseek-ai/dsh/lib/bin.js web",
            entry
        ));
        assert!(!cmdline_owns_entry("node some-other.js", entry));
        assert!(!cmdline_owns_entry(&ours, ""));
    }

    #[test]
    fn only_loopback_urls_with_a_port_are_adoptable() {
        assert!(is_loopback_server("http://127.0.0.1:54732"));
        assert!(is_loopback_server("http://localhost:3080/"));
        assert!(!is_loopback_server("http://127.0.0.1"));
        assert!(!is_loopback_server("http://example.com:8080"));
        assert!(!is_loopback_server("https://127.0.0.1:54732"));
    }

    #[test]
    fn sidecar_record_round_trips() {
        let record = Record {
            pid: 4242,
            entry: "/opt/dsh/lib/bin.js".into(),
            url: Some("http://127.0.0.1:41234".into()),
            proxy: Some("system||".into()),
        };
        let body = serde_json::to_string(&record).unwrap();
        let parsed: Record = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed, record);
        let legacy: Record = serde_json::from_str(r#"{"pid":1,"entry":"/dsh.js"}"#).unwrap();
        assert_eq!(legacy.url, None);
        assert_eq!(legacy.proxy, None);
    }

    #[test]
    fn persist_atomic_replaces_an_existing_file() {
        let dir = std::env::temp_dir().join(format!(
            "oardsh-sidecar-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("oardsh.sidecar.json");
        persist_atomic(&path, br#"{"pid":1,"entry":"/old.js"}"#).unwrap();
        persist_atomic(&path, br#"{"pid":2,"entry":"/new.js"}"#).unwrap();
        let body = fs::read_to_string(&path).unwrap();
        let parsed: Record = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.pid, 2);
        assert_eq!(parsed.entry, "/new.js");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn recover_backup_restores_a_missing_primary() {
        let dir = std::env::temp_dir().join(format!(
            "oardsh-bak-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("oardsh.proxy.json");
        let bak = path.with_extension("bak");
        fs::write(&bak, br#"{"mode":"off"}"#).unwrap();
        recover_backup(&path);
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"mode":"off"}"#);
        assert!(!bak.exists());
        let _ = fs::remove_dir_all(dir);
    }
}
