use std::env;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::sidecar;

const LOOPBACK: &[&str] = &["localhost", "127.0.0.1", "::1", "[::1]"];
/// Well under the Windows environment-block limit, so a huge paste cannot
/// make the next spawn fail and lock the user out of Settings.
const NO_PROXY_MAX: usize = 2048;
const MERGED_NO_PROXY_MAX: usize = 4096;
const PROXY_VARS: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ProxyMode {
    #[default]
    System,
    Off,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProxyConfig {
    pub mode: ProxyMode,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub no_proxy: String,
}

#[derive(Debug, Clone, Default)]
struct InheritedProxy {
    http: String,
    https: String,
    all: String,
    no_proxy: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyView {
    #[serde(flatten)]
    pub config: ProxyConfig,
    /// Node `fetch` honors `NODE_USE_ENV_PROXY` only on 22.21+ / 24+.
    pub fetch_proxy: bool,
}

/// Saved config. Missing or unreadable file means inherit the process env.
pub fn load() -> ProxyConfig {
    let Some(path) = config_path() else {
        return ProxyConfig::default();
    };
    sidecar::recover_backup(&path);
    let Ok(body) = std::fs::read_to_string(path) else {
        return ProxyConfig::default();
    };
    serde_json::from_str(&body).unwrap_or_default()
}

pub fn apply(command: &mut Command) {
    apply_ops(command, &env_ops(&load(), &inherited_proxy()));
}

/// Env mutations for the dsh child. `None` unsets the variable so a saved
/// Off cannot inherit a proxy from the oardsh process.
fn env_ops(
    config: &ProxyConfig,
    inherited: &InheritedProxy,
) -> Vec<(&'static str, Option<String>)> {
    match config.mode {
        ProxyMode::Off => PROXY_VARS
            .iter()
            .copied()
            .map(|name| (name, None))
            .chain(std::iter::once(("NODE_USE_ENV_PROXY", None)))
            .collect(),
        ProxyMode::System => {
            let http = if inherited.http.is_empty() {
                inherited.all.clone()
            } else {
                inherited.http.clone()
            };
            let https = if inherited.https.is_empty() {
                inherited.all.clone()
            } else {
                inherited.https.clone()
            };
            let mut ops = vec![("NODE_USE_ENV_PROXY", Some("1".into()))];
            // A legitimate inherited bypass list can exceed the spawn bound.
            // Leave the launch variables alone rather than dropping hosts.
            if let Some(no_proxy) = merge_no_proxy(&inherited.no_proxy) {
                ops.push(("NO_PROXY", Some(no_proxy.clone())));
                ops.push(("no_proxy", Some(no_proxy)));
            }
            if !http.is_empty() {
                ops.push(("HTTP_PROXY", Some(http.clone())));
                ops.push(("http_proxy", Some(http)));
            }
            if !https.is_empty() {
                ops.push(("HTTPS_PROXY", Some(https.clone())));
                ops.push(("https_proxy", Some(https)));
            }
            ops
        }
        ProxyMode::Manual => {
            let url = config.url.trim().to_string();
            let no_proxy = merge_no_proxy(&config.no_proxy).unwrap_or_else(|| LOOPBACK.join(","));
            vec![
                ("NODE_USE_ENV_PROXY", Some("1".into())),
                ("HTTP_PROXY", Some(url.clone())),
                ("HTTPS_PROXY", Some(url.clone())),
                ("ALL_PROXY", Some(url.clone())),
                ("http_proxy", Some(url.clone())),
                ("https_proxy", Some(url.clone())),
                ("all_proxy", Some(url)),
                ("NO_PROXY", Some(no_proxy.clone())),
                ("no_proxy", Some(no_proxy)),
            ]
        }
    }
}

fn apply_ops(command: &mut Command, ops: &[(&str, Option<String>)]) {
    for (name, value) in ops {
        match value {
            Some(value) => {
                command.env(name, value);
            }
            None => {
                command.env_remove(name);
            }
        }
    }
}

fn first_env(names: &[&str]) -> String {
    for name in names {
        if let Ok(value) = env::var(name) {
            if !value.trim().is_empty() {
                return value;
            }
        }
    }
    String::new()
}

fn inherited_proxy() -> InheritedProxy {
    let no_upper = env::var("NO_PROXY").unwrap_or_default();
    let no_lower = env::var("no_proxy").unwrap_or_default();
    let no_proxy = if no_upper.is_empty() {
        no_lower
    } else if no_lower.is_empty() {
        no_upper
    } else {
        format!("{no_upper},{no_lower}")
    };
    InheritedProxy {
        http: first_env(&["http_proxy", "HTTP_PROXY"]),
        https: first_env(&["https_proxy", "HTTPS_PROXY"]),
        all: first_env(&["all_proxy", "ALL_PROXY"]),
        no_proxy,
    }
}

pub fn fingerprint(config: &ProxyConfig) -> String {
    fingerprint_with(config, &inherited_proxy())
}

fn fingerprint_with(config: &ProxyConfig, inherited: &InheritedProxy) -> String {
    match config.mode {
        // System traffic follows the launch environment, so adopt must
        // refuse a leftover child after HTTP_PROXY / NO_PROXY change.
        ProxyMode::System => format!(
            "system|{}|{}|{}|{}",
            inherited.http, inherited.https, inherited.all, inherited.no_proxy
        ),
        ProxyMode::Off => "off".into(),
        ProxyMode::Manual => format!("manual|{}|{}", config.url, config.no_proxy),
    }
}

/// Loopback plus `user`. `None` if every user host cannot be kept under
/// `MERGED_NO_PROXY_MAX` — callers must not drop the suffix.
fn merge_no_proxy(user: &str) -> Option<String> {
    let mut parts: Vec<String> = LOOPBACK.iter().map(|item| (*item).to_string()).collect();
    for extra in user.split(',') {
        let trimmed = extra.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !parts
            .iter()
            .any(|known| known.eq_ignore_ascii_case(trimmed))
        {
            parts.push(trimmed.to_string());
        }
    }
    let merged = parts.join(",");
    (merged.len() <= MERGED_NO_PROXY_MAX).then_some(merged)
}

fn validate_proxy_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("A proxy URL is required".into());
    }
    if trimmed.len() > 512 {
        return Err("Proxy URL is too long".into());
    }
    let parsed = tauri::Url::parse(trimmed).map_err(|_| "Proxy URL is not valid".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Use an http:// or https:// proxy URL".into());
    }
    if parsed.host_str().is_none() {
        return Err("Proxy URL needs a host".into());
    }
    Ok(trimmed.to_string())
}

fn save(config: &ProxyConfig) -> Result<(), String> {
    let path = config_path().ok_or_else(|| "Unable to resolve DSH_HOME".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let body = serde_json::to_vec(config).map_err(|err| err.to_string())?;
    sidecar::persist_atomic(&path, &body)
        .map_err(|err| format!("Failed to save proxy settings: {err}"))
}

fn config_path() -> Option<std::path::PathBuf> {
    paths::resolve_dsh_home().map(|home| std::path::PathBuf::from(home).join("oardsh.proxy.json"))
}

fn redact_proxy_url(raw: &str) -> String {
    let Ok(mut parsed) = tauri::Url::parse(raw.trim()) else {
        return raw.to_string();
    };
    if parsed.password().is_some() {
        let _ = parsed.set_password(Some("****"));
    }
    parsed.to_string()
}

fn view(app: &tauri::AppHandle, config: ProxyConfig) -> ProxyView {
    let mut config = config;
    if !config.url.is_empty() {
        config.url = redact_proxy_url(&config.url);
    }
    ProxyView {
        config,
        fetch_proxy: node_supports_env_proxy(app),
    }
}

#[tauri::command]
pub fn proxy_config(app: tauri::AppHandle) -> ProxyView {
    view(&app, load())
}

fn validate_no_proxy(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.len() > NO_PROXY_MAX {
        return Err("The no-proxy list is too long".into());
    }
    Ok(trimmed.to_string())
}

/// `NODE_USE_ENV_PROXY` landed in 22.21 and 24.0. Node 23 never got it.
fn node_env_proxy_ok(version: &str) -> bool {
    let trimmed = version.trim().trim_start_matches('v');
    let mut parts = trimmed.split(|c: char| !c.is_ascii_digit());
    let major = parts
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let minor = parts
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    major >= 24 || (major == 22 && minor >= 21)
}

fn node_supports_env_proxy(app: &tauri::AppHandle) -> bool {
    let Ok(node) = paths::resolve_node(app) else {
        return false;
    };
    let mut probe = Command::new(node);
    probe.args(["-p", "process.versions.node"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        probe.creation_flags(0x0800_0000);
    }
    let Ok(output) = probe.output() else {
        return false;
    };
    output.status.success() && node_env_proxy_ok(&String::from_utf8_lossy(&output.stdout))
}

#[tauri::command]
pub fn set_proxy_config(
    app: tauri::AppHandle,
    mode: ProxyMode,
    url: String,
    no_proxy: String,
) -> Result<ProxyView, String> {
    let stored = load();
    let submitted = url.trim();
    if submitted.len() > 512 {
        return Err("Proxy URL is too long".into());
    }
    let mut config = ProxyConfig {
        mode,
        url: if !stored.url.is_empty() && submitted == redact_proxy_url(&stored.url) {
            stored.url.clone()
        } else {
            submitted.to_string()
        },
        no_proxy: validate_no_proxy(&no_proxy)?,
    };
    if config.mode == ProxyMode::Manual {
        config.url = validate_proxy_url(&config.url)?;
        if !node_supports_env_proxy(&app) {
            return Err(
                "Manual proxy needs Node 22.21 or later, or a packaged oardsh runtime.".into(),
            );
        }
    }
    if !confirm_apply(&app) {
        return Err("Cancelled".into());
    }
    save(&config)?;
    crate::engine::restart_from_menu(&app);
    Ok(view(&app, config))
}

fn confirm_apply(app: &tauri::AppHandle) -> bool {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let locale = crate::i18n::system_locale();
    app.dialog()
        .message(crate::i18n::translate(locale, "proxy.confirm.body"))
        .title(crate::i18n::translate(locale, "proxy.confirm.title"))
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancel)
        .blocking_show()
}

#[cfg(test)]
mod tests {
    use super::{
        env_ops, fingerprint_with, merge_no_proxy, node_env_proxy_ok, redact_proxy_url,
        validate_no_proxy, validate_proxy_url, InheritedProxy, ProxyConfig, ProxyMode,
        NO_PROXY_MAX,
    };

    #[test]
    fn rejects_non_http_proxy_urls() {
        assert!(validate_proxy_url("").is_err());
        assert!(validate_proxy_url("ftp://proxy.example:8080").is_err());
        assert!(validate_proxy_url("not a url").is_err());
        assert_eq!(
            validate_proxy_url("  http://127.0.0.1:7890  ").unwrap(),
            "http://127.0.0.1:7890"
        );
        assert!(validate_proxy_url("http://user:pass@127.0.0.1:7890").is_ok());
    }

    #[test]
    fn loopback_is_never_proxied() {
        let merged = merge_no_proxy("example.com, 127.0.0.1").unwrap();
        assert!(merged.contains("localhost"));
        assert!(merged.contains("127.0.0.1"));
        assert!(merged.contains("example.com"));
        assert_eq!(merged.matches("127.0.0.1").count(), 1);
    }

    #[test]
    fn rejects_an_oversized_no_proxy_list() {
        assert!(validate_no_proxy("corp.local").is_ok());
        assert!(validate_no_proxy(&"a".repeat(NO_PROXY_MAX + 1)).is_err());
    }

    #[test]
    fn env_proxy_needs_node_22_21_or_24() {
        assert!(!node_env_proxy_ok("18.20.8"));
        assert!(!node_env_proxy_ok("20.19.0"));
        assert!(!node_env_proxy_ok("v22.20.2"));
        assert!(!node_env_proxy_ok("23.11.0"));
        assert!(node_env_proxy_ok("v22.21.0"));
        assert!(node_env_proxy_ok("24.0.0"));
        assert!(node_env_proxy_ok("24.19.0"));
    }

    #[test]
    fn off_clears_proxy_vars() {
        let ops = env_ops(
            &ProxyConfig {
                mode: ProxyMode::Off,
                ..ProxyConfig::default()
            },
            &InheritedProxy::default(),
        );
        assert!(ops
            .iter()
            .any(|(name, value)| *name == "HTTP_PROXY" && value.is_none()));
        assert!(ops
            .iter()
            .any(|(name, value)| *name == "NODE_USE_ENV_PROXY" && value.is_none()));
    }

    #[test]
    fn manual_sets_node_env_proxy() {
        let ops = env_ops(
            &ProxyConfig {
                mode: ProxyMode::Manual,
                url: "http://127.0.0.1:7890".into(),
                no_proxy: "corp.local".into(),
            },
            &InheritedProxy::default(),
        );
        let https = ops
            .iter()
            .find(|(name, _)| *name == "HTTPS_PROXY")
            .and_then(|(_, value)| value.as_deref());
        assert_eq!(https, Some("http://127.0.0.1:7890"));
        let no_proxy = ops
            .iter()
            .find(|(name, _)| *name == "NO_PROXY")
            .and_then(|(_, value)| value.as_deref())
            .unwrap();
        assert!(no_proxy.contains("localhost"));
        assert!(no_proxy.contains("corp.local"));
        assert_eq!(
            ops.iter()
                .find(|(name, _)| *name == "NODE_USE_ENV_PROXY")
                .and_then(|(_, value)| value.as_deref()),
            Some("1")
        );
    }

    #[test]
    fn system_maps_all_proxy_when_protocol_vars_are_absent() {
        let ops = env_ops(
            &ProxyConfig {
                mode: ProxyMode::System,
                ..ProxyConfig::default()
            },
            &InheritedProxy {
                all: "http://127.0.0.1:7890".into(),
                ..InheritedProxy::default()
            },
        );
        let https = ops
            .iter()
            .find(|(name, _)| *name == "HTTPS_PROXY")
            .and_then(|(_, value)| value.as_deref());
        assert_eq!(https, Some("http://127.0.0.1:7890"));
        let http = ops
            .iter()
            .find(|(name, _)| *name == "HTTP_PROXY")
            .and_then(|(_, value)| value.as_deref());
        assert_eq!(http, Some("http://127.0.0.1:7890"));
        let no_proxy = ops
            .iter()
            .find(|(name, _)| *name == "NO_PROXY")
            .and_then(|(_, value)| value.as_deref())
            .unwrap();
        assert!(no_proxy.contains("localhost"));
    }

    #[test]
    fn system_fills_only_the_missing_protocol_from_all_proxy() {
        let ops = env_ops(
            &ProxyConfig {
                mode: ProxyMode::System,
                ..ProxyConfig::default()
            },
            &InheritedProxy {
                http: "http://already:8080".into(),
                all: "http://127.0.0.1:7890".into(),
                ..InheritedProxy::default()
            },
        );
        let http = ops
            .iter()
            .find(|(name, _)| *name == "HTTP_PROXY")
            .and_then(|(_, value)| value.as_deref());
        assert_eq!(http, Some("http://already:8080"));
        let http_lc = ops
            .iter()
            .find(|(name, _)| *name == "http_proxy")
            .and_then(|(_, value)| value.as_deref());
        assert_eq!(http_lc, Some("http://already:8080"));
        let https = ops
            .iter()
            .find(|(name, _)| *name == "HTTPS_PROXY")
            .and_then(|(_, value)| value.as_deref());
        assert_eq!(https, Some("http://127.0.0.1:7890"));
    }

    #[test]
    fn oversized_inherited_no_proxy_is_not_rewritten() {
        let huge = (0..400)
            .map(|i| format!("host{i}.internal.example"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(merge_no_proxy(&huge).is_none());
        let ops = env_ops(
            &ProxyConfig {
                mode: ProxyMode::System,
                ..ProxyConfig::default()
            },
            &InheritedProxy {
                no_proxy: huge,
                ..InheritedProxy::default()
            },
        );
        assert!(ops
            .iter()
            .all(|(name, _)| *name != "NO_PROXY" && *name != "no_proxy"));
        assert!(ops
            .iter()
            .any(|(name, value)| *name == "NODE_USE_ENV_PROXY" && value.as_deref() == Some("1")));
    }

    #[test]
    fn fingerprint_includes_inherited_system_proxy() {
        let config = ProxyConfig {
            mode: ProxyMode::System,
            ..ProxyConfig::default()
        };
        let first = fingerprint_with(
            &config,
            &InheritedProxy {
                http: "http://proxy-a:8080".into(),
                ..InheritedProxy::default()
            },
        );
        let second = fingerprint_with(
            &config,
            &InheritedProxy {
                http: "http://proxy-b:8080".into(),
                ..InheritedProxy::default()
            },
        );
        assert_ne!(first, second);
        let off = ProxyConfig {
            mode: ProxyMode::Off,
            ..ProxyConfig::default()
        };
        assert_eq!(
            fingerprint_with(
                &off,
                &InheritedProxy {
                    http: "http://proxy-a:8080".into(),
                    ..InheritedProxy::default()
                }
            ),
            fingerprint_with(
                &off,
                &InheritedProxy {
                    http: "http://proxy-b:8080".into(),
                    ..InheritedProxy::default()
                }
            )
        );
    }

    #[test]
    fn redacts_proxy_passwords() {
        assert_eq!(
            redact_proxy_url("http://user:secret@127.0.0.1:7890"),
            "http://user:****@127.0.0.1:7890/"
        );
        assert_eq!(
            redact_proxy_url("http://127.0.0.1:7890"),
            "http://127.0.0.1:7890/"
        );
    }
}
