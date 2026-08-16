use std::env;

use serde_json::Value;

const EN: &str = include_str!("../locales/en.json");
const ZH_CN: &str = include_str!("../locales/zh-CN.json");

pub fn translate(locale: &str, key: &str) -> String {
    let catalog = if normalize_locale(locale) == "zh-CN" {
        ZH_CN
    } else {
        EN
    };
    serde_json::from_str::<Value>(catalog)
        .ok()
        .and_then(|value| value.get(key).and_then(Value::as_str).map(str::to_string))
        .or_else(|| {
            serde_json::from_str::<Value>(EN)
                .ok()
                .and_then(|value| value.get(key).and_then(Value::as_str).map(str::to_string))
        })
        .unwrap_or_else(|| key.to_string())
}

pub fn normalize_locale(locale: &str) -> &'static str {
    if locale.trim().to_ascii_lowercase().starts_with("zh") {
        "zh-CN"
    } else {
        "en"
    }
}

/// Tray labels follow the OS language. The boot screen does the same; dsh's
/// own language preference is not available until the server is up.
pub fn system_locale() -> &'static str {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = env::var(key) {
            if !value.is_empty() && value != "C" && value != "POSIX" {
                return normalize_locale(&value);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("defaults")
            .args(["read", "-g", "AppleLocale"])
            .output()
        {
            if output.status.success() {
                let value = String::from_utf8_lossy(&output.stdout);
                return normalize_locale(value.trim());
            }
        }
    }
    "en"
}

#[cfg(test)]
mod tests {
    use super::{normalize_locale, translate};

    #[test]
    fn normalizes_and_falls_back() {
        assert_eq!(normalize_locale("zh-Hans-CN"), "zh-CN");
        assert_eq!(normalize_locale("fr-FR"), "en");
        assert_eq!(
            translate("en", "notification.completed.title"),
            "dsh finished the current turn"
        );
        assert_eq!(translate("zh-CN", "tray.quit"), "退出 oardsh");
        assert_eq!(
            translate("en", "notification.crash.title"),
            "dsh stopped unexpectedly"
        );
        assert_eq!(translate("zh-CN", "menu.file"), "文件");
        assert_eq!(
            translate("en", "notification.tray.title"),
            "oardsh is still running"
        );
    }
}
