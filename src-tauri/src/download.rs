use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tauri::webview::DownloadEvent;
use tauri::{Webview, Wry};

#[cfg(not(target_os = "linux"))]
use tauri::Manager;

use crate::i18n;

/// dsh's session-log export is a plain browser download: an `<a download>`
/// click that a real browser hands to its download manager. wry drops such
/// navigations unless the shell registered a handler, so every download in
/// the dsh page is routed through here — the user picks where it lands, and a
/// toast in the page says where it landed once it finishes.
pub fn handle(webview: Webview<Wry>, event: DownloadEvent<'_>) -> bool {
    match event {
        DownloadEvent::Requested { url, destination } => {
            match pick_destination(&webview, &url, destination) {
                Some(path) => {
                    remember(url.as_str(), path.clone());
                    *destination = staging_destination(url.as_str(), path);
                    true
                }
                None => {
                    mark_cancelled(url.as_str());
                    false
                }
            }
        }
        DownloadEvent::Finished { url, path, success } => {
            announce(&webview, &url, path, success);
            true
        }
        _ => true,
    }
}

/// Finished carries no path on macOS (WKWebView API limitation), so remember
/// what the user chose — a FIFO per URL, because exporting the same session
/// again while its first ZIP still streams must not overwrite the first
/// choice. Completions are matched to choices by order, the only identity
/// wry exposes; dsh itself serialises a session's exports until its save
/// starts, so ordering only escapes when two transfers of one URL finish
/// out of turn, and the cost is two real destinations announced swapped.
/// Bounded, because a download that never finishes would otherwise leak its
/// entry.
static CHOSEN: OnceLock<Mutex<HashMap<String, VecDeque<PathBuf>>>> = OnceLock::new();

/// Save-dialog cancellations. The shell rejects such downloads on purpose,
/// yet WebView2 still reports them back as failed downloads; remembering the
/// cancellation keeps the failure toast off what was a deliberate choice.
static CANCELLED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn chosen() -> &'static Mutex<HashMap<String, VecDeque<PathBuf>>> {
    CHOSEN.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cancelled() -> &'static Mutex<HashSet<String>> {
    CANCELLED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn remember(url: &str, path: PathBuf) {
    let mut chosen = chosen().lock().unwrap_or_else(|err| err.into_inner());
    if chosen.len() >= 32 {
        chosen.clear();
    }
    chosen.entry(url.to_string()).or_default().push_back(path);
}

fn take(url: &str) -> Option<PathBuf> {
    chosen()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .get_mut(url)
        .and_then(VecDeque::pop_front)
}

fn mark_cancelled(url: &str) {
    let mut cancelled = cancelled().lock().unwrap_or_else(|err| err.into_inner());
    if cancelled.len() >= 32 {
        cancelled.clear();
    }
    cancelled.insert(url.to_string());
}

fn take_cancelled(url: &str) -> bool {
    cancelled()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .remove(url)
}

/// Linux completion reporting cannot be trusted after any failure (see
/// `announce`); a chosen destination that exists on disk is the truth left.
#[cfg(target_os = "linux")]
fn arrived(url: &str) -> bool {
    chosen()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .get(url)
        .and_then(|queue| queue.front())
        .is_some_and(|path| path.exists())
}

/// WKDownload fails rather than replaces an existing file, so a user who
/// confirms the save panel's replacement prompt would lose the transfer.
/// Stage beside the target instead, and move over it on completion (see
/// `settle`). The staging paths join the same per-URL FIFO as the choices,
/// so a download's staging file and destination are consumed as a pair.
#[cfg(target_os = "macos")]
static PENDING: OnceLock<Mutex<HashMap<String, VecDeque<PathBuf>>>> = OnceLock::new();

#[cfg(target_os = "macos")]
fn pending() -> &'static Mutex<HashMap<String, VecDeque<PathBuf>>> {
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The URL a finished transfer settles at: the user's chosen path directly,
/// unless that path existed — then a fresh hidden sibling to stage through.
#[cfg(target_os = "macos")]
fn staging_destination(url: &str, destination: PathBuf) -> PathBuf {
    if !destination.exists() {
        return destination;
    }
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut staged = destination.clone();
    let mut counter = 0;
    loop {
        counter += 1;
        staged.set_file_name(format!(".{name}.oardsh-{counter}.part"));
        if !staged.exists() {
            break;
        }
    }
    let mut pending = pending().lock().unwrap_or_else(|err| err.into_inner());
    if pending.len() >= 32 {
        pending.clear();
    }
    pending
        .entry(url.to_string())
        .or_default()
        .push_back(staged.clone());
    staged
}

#[cfg(not(target_os = "macos"))]
fn staging_destination(_url: &str, destination: PathBuf) -> PathBuf {
    destination
}

/// Move a finished staging file over the destination it stands in for.
#[cfg(target_os = "macos")]
fn settle(url: &str, destination: Option<PathBuf>) -> Option<PathBuf> {
    let destination = destination?;
    match pending()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .get_mut(url)
        .and_then(VecDeque::pop_front)
    {
        Some(temp) if std::fs::rename(&temp, &destination).is_ok() => Some(destination),
        // The move failed — say where the archive actually is rather than
        // claim a save that did not land.
        Some(temp) => Some(temp),
        None => Some(destination),
    }
}

#[cfg(target_os = "macos")]
fn take_pending(url: &str) -> Option<PathBuf> {
    pending()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .get_mut(url)
        .and_then(VecDeque::pop_front)
}

fn announce(webview: &Webview<Wry>, url: &tauri::Url, path: Option<PathBuf>, success: bool) {
    let locale = i18n::system_locale();
    // wry 0.55's webkitgtk backend tracks download failure on one flag for
    // the whole web context and never resets it, so after any real failure
    // every later successful download arrives here with success == false.
    // No fixed wry is reachable through tauri 2.11 (it pins 0.55.x, and dev
    // carries the same bug); until one is, trust the filesystem on Linux —
    // the cost is a truncated transfer still announcing, which beats every
    // later export reading as failed.
    #[cfg(target_os = "linux")]
    let success = success || arrived(url.as_str());
    // Consume the remembered choice on every completion, reported path or
    // not, so a later download of the same URL never inherits a stale one;
    // the platform's path is only what gets displayed.
    let remembered = take(url.as_str());
    let text = if success {
        // macOS: WKDownload cannot write onto an existing file, so a
        // replacement the user confirmed in the save panel was staged
        // beside the target; move it into place now.
        #[cfg(target_os = "macos")]
        let remembered = settle(url.as_str(), remembered);
        match path.or(remembered) {
            Some(path) => format!(
                "{}: {}",
                i18n::translate(locale, "download.saved"),
                path.display()
            ),
            None => i18n::translate(locale, "download.savedDefault"),
        }
    } else if take_cancelled(url.as_str()) {
        // The user closed the save dialog; the rejection reported here is
        // that choice, not a failure worth announcing.
        return;
    } else {
        #[cfg(target_os = "macos")]
        if let Some(temp) = take_pending(url.as_str()) {
            let _ = std::fs::remove_file(temp);
        }
        i18n::translate(locale, "download.failed")
    };
    let _ = webview.eval(toast_script(&text, success).as_str());
}

/// One idempotent toast in the corner of the dsh page; its CSS variables keep
/// the theme, with neutral fallbacks for a page that lacks them. The path is
/// set via textContent (never innerHTML) so its contents stay inert.
fn toast_script(text: &str, success: bool) -> String {
    let message = serde_json::to_string(text).unwrap_or_default();
    let color = if success {
        "var(--dsw-alias-label-primary,#111)"
    } else {
        "var(--dsw-alias-state-error-primary,#e34948)"
    };
    format!(
        r#"(function(){{var t=document.getElementById('oardsh-download-toast');if(!t){{t=document.createElement('div');t.id='oardsh-download-toast';t.style.cssText='position:fixed;z-index:2147483000;right:16px;bottom:16px;max-width:420px;padding:10px 14px;border-radius:10px;font:12px/18px system-ui,sans-serif;box-shadow:0 6px 20px rgba(0,0,0,.18);background:var(--dsw-alias-bg-layer-1,#fff);border:1px solid var(--dsw-alias-border-l2,rgba(0,0,0,.12));transition:opacity .4s;pointer-events:none';document.body.appendChild(t);}}clearTimeout(t._oardsh);t.style.color='{color}';t.textContent={message};t.style.opacity='1';t._oardsh=setTimeout(function(){{t.style.opacity='0';}},6000);}})()"#
    )
}

/// Each platform constructs a different subset; the unused one is only
/// unreachable, not nonsensical.
#[allow(dead_code)]
/// The `Requested` callback runs on the main event-loop thread and must
/// answer synchronously, which rules out the dialog plugin (its blocking API
/// queues onto the same thread it would then wait on). rfd's sync API works
/// here instead: Windows shows the modal IFileDialog, which pumps its own
/// messages, and macOS runs the save panel in a nested modal loop — both
/// inline on this thread. `None` cancels the download.
#[cfg(not(target_os = "linux"))]
fn pick_destination(
    webview: &Webview<Wry>,
    url: &tauri::Url,
    destination: &Path,
) -> Option<PathBuf> {
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            url.path_segments()
                .and_then(Iterator::last)
                .map(str::to_string)
        })
        .unwrap_or_default();
    let mut dialog = rfd::FileDialog::new();
    if !name.is_empty() {
        dialog = dialog.set_file_name(&name);
    }
    if let Some(window) = webview.app_handle().get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog.save_file()
}

/// rfd schedules its GTK dialog onto the main loop that this callback is
/// blocking, so asking is impossible without a deadlock; keep wry's default
/// destination until a nested-loop dialog replaces it.
#[cfg(target_os = "linux")]
fn pick_destination(
    _webview: &Webview<Wry>,
    _url: &tauri::Url,
    destination: &Path,
) -> Option<PathBuf> {
    Some(destination.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::{mark_cancelled, remember, take, take_cancelled, toast_script};
    use std::path::PathBuf;

    // The maps behind these are process-global and tests run in parallel, so
    // each stakes out its own session.
    #[test]
    fn remembers_the_chosen_path_per_url() {
        let url = "http://127.0.0.1:1/api/session.export?sessionId=single";
        remember(url, PathBuf::from(r"C:\Downloads\a.zip"));
        assert_eq!(take(url), Some(PathBuf::from(r"C:\Downloads\a.zip")));
        assert_eq!(take(url), None);
    }

    #[test]
    fn queues_a_repeated_export_of_the_same_session() {
        let url = "http://127.0.0.1:1/api/session.export?sessionId=queued";
        remember(url, PathBuf::from(r"C:\Downloads\first.zip"));
        remember(url, PathBuf::from(r"C:\Downloads\second.zip"));
        assert_eq!(take(url), Some(PathBuf::from(r"C:\Downloads\first.zip")));
        assert_eq!(take(url), Some(PathBuf::from(r"C:\Downloads\second.zip")));
        assert_eq!(take(url), None);
    }

    #[test]
    fn a_cancelled_save_dialog_is_remembered_once() {
        let url = "http://127.0.0.1:1/api/session.export?sessionId=cancelled";
        assert!(!take_cancelled(url));
        mark_cancelled(url);
        assert!(take_cancelled(url));
        assert!(!take_cancelled(url));
    }

    #[test]
    fn toast_script_embeds_the_path_as_an_escaped_literal() {
        let script = toast_script("已保存到: C:\\Users\\Vince \"q\".zip", false);
        assert!(script.contains("textContent=\"已保存到: C:\\\\Users\\\\Vince \\\"q\\\".zip\""));
        assert!(script.contains("oardsh-download-toast"));
    }
}
