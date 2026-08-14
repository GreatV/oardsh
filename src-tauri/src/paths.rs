use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use tauri::{AppHandle, Manager};

const DSH_ENTRY: &str = "node_modules/@deepseek-ai/dsh/lib/bin.js";

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub path: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub node: Option<Tool>,
    pub npx: Option<Tool>,
    pub dsh: Option<Tool>,
    pub home: Option<String>,
    pub dsh_home: Option<String>,
    pub path: String,
    pub launcher: String,
}

pub fn probe(app: &AppHandle) -> Environment {
    let path = path_with_package_bins(app);
    env::set_var("PATH", &path);
    let node = resolve_node(app)
        .ok()
        .map(|path| tool_from_command(&path, &[], &["-v"]));
    let npx = find_tool("npx", &["--version"]);
    let dsh = bundled_dsh(app).map(|dsh| match resolve_node(app) {
        Ok(node) => tool_from_command(&node, &[dsh.as_os_str()], &["-V"]),
        Err(_) => tool_from_command(&dsh, &[], &["-V"]),
    });
    let home = home_dir().map(|p| p.display().to_string());
    let dsh_home = resolve_dsh_home();
    let launcher = if bundled_dsh(app).is_some() {
        "bundled".to_string()
    } else if dsh.is_some() {
        "dsh".to_string()
    } else {
        "missing".to_string()
    };
    Environment {
        node,
        npx,
        dsh,
        home,
        dsh_home,
        path: path.to_string_lossy().into_owned(),
        launcher,
    }
}

pub fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub fn resolve_dsh_home() -> Option<String> {
    if let Ok(value) = env::var("DSH_HOME") {
        if !value.trim().is_empty() {
            return Some(value);
        }
    }
    home_dir().map(|home| home.join(".dsh").display().to_string())
}

pub fn augmented_path() -> OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = home_dir() {
        push_nvm_bins(&home, &mut dirs);
        push_fnm_bins(&home, &mut dirs);
        push_existing(
            &mut dirs,
            [
                home.join(".volta/bin"),
                home.join(".asdf/shims"),
                home.join(".local/share/pnpm"),
                home.join("Library/pnpm"),
                home.join(".local/bin"),
                home.join(".cargo/bin"),
                home.join(".deno/bin"),
                home.join(".yarn/bin"),
                home.join(".nodenv/shims"),
            ],
        );
    }
    push_existing(
        &mut dirs,
        [
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/opt/homebrew/opt/node/bin"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/usr/local/opt/node/bin"),
            PathBuf::from(r"C:\Program Files\nodejs"),
        ],
    );

    let mut parts: Vec<OsString> = dirs.into_iter().map(OsString::from).collect();
    if let Some(existing) = env::var_os("PATH") {
        for part in env::split_paths(&existing) {
            if !parts.iter().any(|known| Path::new(known) == part) {
                parts.push(part.into());
            }
        }
    }
    env::join_paths(parts).unwrap_or_else(|_| env::var_os("PATH").unwrap_or_default())
}

pub fn find_binary(name: &str) -> Option<PathBuf> {
    which(name, &augmented_path())
}

pub fn bundled_node(app: &AppHandle) -> Option<PathBuf> {
    let names = if cfg!(windows) {
        ["node.exe", "node"]
    } else {
        ["node", "node.exe"]
    };
    let mut dirs = Vec::new();
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.to_path_buf());
        }
    }
    if let Ok(resource) = app.path().resource_dir() {
        dirs.push(resource.join("runtime"));
        dirs.push(resource);
    }
    dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/runtime"));
    for dir in dirs {
        for name in names {
            let candidate = dir.join(name);
            if is_executable(&candidate) {
                return fs::canonicalize(candidate).ok();
            }
        }
    }
    None
}

pub fn resolve_node(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(node) = bundled_node(app) {
        return Ok(node);
    }
    find_binary("node").ok_or_else(|| {
        "Node.js was not found. Development needs Node 18+; a packaged app should include a runtime via `npm run prepare-runtime`."
            .into()
    })
}

pub fn bundled_dsh(app: &AppHandle) -> Option<PathBuf> {
    for root in search_roots(app) {
        if let Some(found) = locate_dsh_near(&root) {
            return Some(found);
        }
    }
    None
}

pub fn path_with_package_bins(app: &AppHandle) -> OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(node) = resolve_node(app) {
        if let Some(dir) = node.parent() {
            dirs.push(dir.to_path_buf());
        }
    }
    if let Some(dsh) = bundled_dsh(app) {
        if let Some(bin_dir) = package_bin_dir(&dsh) {
            dirs.push(bin_dir);
        }
    }
    let mut parts: Vec<OsString> = dirs.into_iter().map(OsString::from).collect();
    for part in env::split_paths(&augmented_path()) {
        if !parts.iter().any(|known| Path::new(known) == part) {
            parts.push(part.into());
        }
    }
    env::join_paths(parts).unwrap_or_else(|_| augmented_path())
}

pub fn resolve_launch(
    app: &AppHandle,
    custom_dsh: Option<&str>,
) -> Result<(PathBuf, Vec<String>, String), String> {
    if let Some(custom) = custom_dsh.map(str::trim).filter(|s| !s.is_empty()) {
        let path = PathBuf::from(custom);
        if !path.exists() {
            return Err(format!("Custom dsh path does not exist: {custom}"));
        }
        return Ok((path, vec!["web".into()], format!("{custom} web")));
    }

    if let Some(dsh) = bundled_dsh(app) {
        let display = format!("{} web", dsh.display());
        return Ok((dsh, vec!["web".into()], display));
    }

    Err(
        "Bundled @deepseek-ai/dsh is missing. Run `npm install` in the oardsh project so the package is available in node_modules."
            .into(),
    )
}

fn search_roots(app: &AppHandle) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."));
    roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/dsh"));
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            roots.push(dir.to_path_buf());
            if dir.ends_with("MacOS") {
                roots.push(dir.join("../Resources"));
                roots.push(dir.join("../Resources/dsh"));
            }
        }
    }
    if let Ok(resource) = app.path().resource_dir() {
        roots.push(resource.join("dsh"));
        roots.push(resource);
    }
    if let Ok(cwd) = env::current_dir() {
        roots.push(cwd);
    }
    roots
}

fn locate_dsh_near(root: &Path) -> Option<PathBuf> {
    let mut dir = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    for _ in 0..10 {
        let candidate = dir.join(DSH_ENTRY);
        if candidate.is_file() {
            return fs::canonicalize(candidate).ok();
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    None
}

fn package_bin_dir(dsh_entry: &Path) -> Option<PathBuf> {
    // .../node_modules/@deepseek-ai/dsh/lib/bin.js
    dsh_entry
        .parent()
        .and_then(|lib| lib.parent())
        .and_then(|pkg| pkg.parent())
        .and_then(|scope| scope.parent())
        .map(|node_modules| node_modules.join(".bin"))
        .filter(|dir| dir.is_dir())
}

fn find_tool(name: &str, version_args: &[&str]) -> Option<Tool> {
    let path = which(name, &augmented_path())?;
    Some(tool_from_command(&path, &[], version_args))
}

fn tool_from_command(program: &Path, prefix: &[&OsStr], version_args: &[&str]) -> Tool {
    let version = Command::new(program)
        .args(prefix)
        .args(version_args)
        .env("PATH", augmented_path())
        .output()
        .ok()
        .and_then(|output| {
            let text = if output.stdout.is_empty() {
                String::from_utf8_lossy(&output.stderr).into_owned()
            } else {
                String::from_utf8_lossy(&output.stdout).into_owned()
            };
            let line = text.lines().next().unwrap_or_default().trim();
            if line.is_empty() {
                None
            } else {
                Some(line.to_string())
            }
        });
    let path = prefix
        .last()
        .map(PathBuf::from)
        .unwrap_or_else(|| program.to_path_buf());
    Tool {
        path: path.display().to_string(),
        version,
    }
}

fn which(name: &str, path_value: &OsString) -> Option<PathBuf> {
    for dir in env::split_paths(path_value) {
        for candidate in binary_names(name) {
            let path = dir.join(candidate);
            if is_executable(&path) {
                return Some(path);
            }
        }
    }
    None
}

fn binary_names(name: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![
            format!("{name}.cmd"),
            format!("{name}.exe"),
            format!("{name}.bat"),
            name.to_string(),
        ]
    } else {
        vec![name.to_string()]
    }
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn push_existing<const N: usize>(dirs: &mut Vec<PathBuf>, candidates: [PathBuf; N]) {
    for path in candidates {
        if path.is_dir() && !dirs.iter().any(|known| known == &path) {
            dirs.push(path);
        }
    }
}

fn push_nvm_bins(home: &Path, dirs: &mut Vec<PathBuf>) {
    let versions = home.join(".nvm/versions/node");
    let mut found = collect_version_bins(&versions, "bin");
    found.sort_by(|a, b| version_key(&b.0).cmp(&version_key(&a.0)));
    for (_, bin) in found {
        if !dirs.iter().any(|known| known == &bin) {
            dirs.push(bin);
        }
    }
}

fn push_fnm_bins(home: &Path, dirs: &mut Vec<PathBuf>) {
    let versions = home.join(".fnm/node-versions");
    let mut found = collect_version_bins(&versions, "installation/bin");
    found.sort_by(|a, b| version_key(&b.0).cmp(&version_key(&a.0)));
    for (_, bin) in found {
        if !dirs.iter().any(|known| known == &bin) {
            dirs.push(bin);
        }
    }
}

fn collect_version_bins(root: &Path, suffix: &str) -> Vec<(String, PathBuf)> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let bin = entry.path().join(suffix);
        if bin.is_dir() {
            found.push((name, bin));
        }
    }
    found
}

fn version_key(name: &str) -> (u64, u64, u64) {
    let trimmed = name.trim_start_matches('v');
    let mut parts = trimmed.split(|c: char| !c.is_ascii_digit());
    let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

#[allow(dead_code)]
pub fn short_timeout() -> Duration {
    Duration::from_secs(2)
}
