use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

const RECENT_LIMIT: usize = 8;
const DEFAULT_PORT: u16 = 3080;
const DEFAULT_HOST: &str = "127.0.0.1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub workspace: Option<String>,
    #[serde(default)]
    pub recent_workspaces: Vec<String>,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_host")]
    pub host: String,
    pub dsh_path: Option<String>,
    #[serde(default = "default_true")]
    pub auto_launch: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            workspace: None,
            recent_workspaces: Vec::new(),
            port: DEFAULT_PORT,
            host: DEFAULT_HOST.to_string(),
            dsh_path: None,
            auto_launch: true,
        }
    }
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

fn default_host() -> String {
    DEFAULT_HOST.to_string()
}

fn default_true() -> bool {
    true
}

pub fn load(app: &AppHandle) -> Settings {
    let path = settings_file(app);
    let Ok(bytes) = std::fs::read(&path) else {
        return Settings::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

pub fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let path = settings_file(app);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

pub fn remember_workspace(settings: &mut Settings, workspace: &str) {
    settings.workspace = Some(workspace.to_string());
    settings
        .recent_workspaces
        .retain(|item| item != workspace);
    settings.recent_workspaces.insert(0, workspace.to_string());
    settings.recent_workspaces.truncate(RECENT_LIMIT);
}

fn settings_file(app: &AppHandle) -> PathBuf {
    app.path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("settings.json")
}
