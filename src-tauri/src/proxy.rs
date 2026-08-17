use std::env;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::sidecar;

const LOOPBACK: &[&str] = &["localhost", "127.0.0.1", "::1", "[::1]"];
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

/// Saved config. Missing or unreadable file means inherit the process env.
pub fn load() -> ProxyConfig {
    let Some(path) = config_path() else {
        return ProxyConfig::default();
    };
    let Ok(body) = std::fs::read_to_string(path) else {
        return ProxyConfig::default();
    };
    serde_json::from_str(&body).unwrap_or_default()
}

pub fn apply(command: &mut Command) {
    apply_ops(command, &env_ops(&load(), inherited_no_proxy()));
}

/// Env mutations for the dsh child. `None` unsets the variable so a saved
/// Off cannot inherit a proxy from the oardsh process.
fn env_ops(
    config: &ProxyConfig,
    inherited_no_proxy: String,
) -> Vec<(&'static str, Option<String>)> {
    match config.mode {
        ProxyMode::Off => PROXY_VARS
            .iter()
            .copied()
            .map(|name| (name, None))
            .chain(std::iter::once(("NODE_USE_ENV_PROXY", None)))
            .collect(),
        ProxyMode::System => vec![
            ("NODE_USE_ENV_PROXY", Some("1".into())),
            ("NO_PROXY", Some(merge_no_proxy(&inherited_no_proxy))),
            ("no_proxy", Some(merge_no_proxy(&inherited_no_proxy))),
        ],
        ProxyMode::Manual => {
            let url = config.url.trim().to_string();
            let no_proxy = merge_no_proxy(&config.no_proxy);
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

fn inherited_no_proxy() -> String {
    env::var("NO_PROXY")
        .or_else(|_| env::var("no_proxy"))
        .unwrap_or_default()
}

fn merge_no_proxy(user: &str) -> String {
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
    parts.join(",")
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
    sidecar::persist_atomic(&path, &body);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    if !path.is_file() {
        return Err("Failed to save proxy settings".into());
    }
    Ok(())
}

fn config_path() -> Option<std::path::PathBuf> {
    paths::resolve_dsh_home().map(|home| std::path::PathBuf::from(home).join("oardsh.proxy.json"))
}

#[tauri::command]
pub fn proxy_config() -> ProxyConfig {
    load()
}

#[tauri::command]
pub fn set_proxy_config(
    mode: ProxyMode,
    url: String,
    no_proxy: String,
) -> Result<ProxyConfig, String> {
    let mut config = ProxyConfig {
        mode,
        url: url.trim().to_string(),
        no_proxy: no_proxy.trim().to_string(),
    };
    if config.mode == ProxyMode::Manual {
        config.url = validate_proxy_url(&config.url)?;
    }
    save(&config)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::{env_ops, merge_no_proxy, validate_proxy_url, ProxyConfig, ProxyMode};

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
        let merged = merge_no_proxy("example.com, 127.0.0.1");
        assert!(merged.contains("localhost"));
        assert!(merged.contains("127.0.0.1"));
        assert!(merged.contains("example.com"));
        assert_eq!(merged.matches("127.0.0.1").count(), 1);
    }

    #[test]
    fn off_clears_proxy_vars() {
        let ops = env_ops(
            &ProxyConfig {
                mode: ProxyMode::Off,
                ..ProxyConfig::default()
            },
            String::new(),
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
            String::new(),
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
}
