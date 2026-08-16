use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

const DSH_ENTRY: &str = "node_modules/@deepseek-ai/dsh/lib/bin.js";
const PLUGIN_FILES: &[&str] = &["package.json", "lib/index.js", "lib/client.js"];

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
                return canonicalize_for_child(&candidate);
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

/// Resolve the dsh launcher, so callers never learn its package layout.
pub fn resolve_dsh(app: &AppHandle) -> Result<PathBuf, String> {
    bundled_dsh(app).ok_or_else(|| {
        "Bundled @deepseek-ai/dsh is missing. Run `npm install` and try again.".into()
    })
}

pub fn desktop_patch(app: &AppHandle) -> Result<PathBuf, String> {
    let mut candidates =
        vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/oardsh.patch.yml")];
    if let Ok(resource) = app.path().resource_dir() {
        candidates.push(resource.join("oardsh.patch.yml"));
        candidates.push(resource.join("resources/oardsh.patch.yml"));
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .and_then(canonicalize_for_child)
        .ok_or_else(|| "oardsh dsh plugin patch is missing".to_string())
}

/// Stage the oardsh package beside dsh's shared profile modules, where dsh
/// resolves Loader entries from.
pub fn ensure_desktop_plugin(app: &AppHandle) -> Result<(), String> {
    let mut sources = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../packages/oardsh-dsh-plugin"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/dsh/plugins/oardsh-dsh-plugin"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources/dsh/node_modules/@oardsh/dsh-plugin"),
    ];
    if let Ok(resource) = app.path().resource_dir() {
        // node_modules holds a symlink that may not survive the bundler, so
        // prefer the real directory prepare-runtime copies to dsh/plugins.
        sources.push(resource.join("dsh/plugins/oardsh-dsh-plugin"));
        sources.push(resource.join("dsh/node_modules/@oardsh/dsh-plugin"));
    }
    let source = sources
        .into_iter()
        .find(|path| path.join("lib/client.js").is_file())
        .ok_or_else(|| "oardsh dsh client plugin is missing".to_string())?;
    let home = resolve_dsh_home()
        .map(PathBuf::from)
        .ok_or_else(|| "Unable to resolve DSH_HOME".to_string())?;
    let target = home.join("profiles/node_modules/@oardsh/dsh-plugin");
    // package.json carries `dsh.client.inject`, so comparing only the generated
    // client would miss a change to which dsh modules the plugin needs.
    let up_to_date = PLUGIN_FILES.iter().all(|name| {
        matches!(
            (fs::read(target.join(name)), fs::read(source.join(name))),
            (Ok(staged), Ok(origin)) if staged == origin
        )
    });
    if up_to_date {
        return Ok(());
    }
    if target.exists() {
        fs::remove_dir_all(&target).map_err(|err| {
            format!(
                "Failed to refresh managed desktop plugin {}: {err}",
                target.display()
            )
        })?;
    }
    fs::create_dir_all(target.join("lib")).map_err(|err| err.to_string())?;
    for name in PLUGIN_FILES {
        fs::copy(source.join(name), target.join(name))
            .map_err(|err| format!("Failed to stage desktop plugin file {name}: {err}"))?;
    }
    Ok(())
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
            return canonicalize_for_child(&candidate);
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    None
}

/// Resolve a path for a child process. Windows `canonicalize` returns
/// `\\?\C:\...`; Node 22+ `realpathSync` parses that as UNC and `lstat`s `C:`.
fn canonicalize_for_child(path: impl AsRef<Path>) -> Option<PathBuf> {
    fs::canonicalize(path.as_ref())
        .ok()
        .map(strip_windows_namespace)
}

/// Drop the Windows extended-length prefix so Node and other Win32 programs
/// see a normal drive path. `\\?\C:\foo` → `C:\foo`, `\\?\UNC\s\sh\foo` →
/// `\\s\sh\foo`. Other paths are left unchanged.
pub(crate) fn strip_windows_namespace(path: PathBuf) -> PathBuf {
    path.to_str()
        .and_then(strip_windows_namespace_str)
        .map(PathBuf::from)
        .unwrap_or(path)
}

fn strip_windows_namespace_str(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let is_namespace = bytes.len() >= 4
        && matches!(bytes[0], b'\\' | b'/')
        && matches!(bytes[1], b'\\' | b'/')
        && bytes[2] == b'?'
        && matches!(bytes[3], b'\\' | b'/');
    if !is_namespace {
        return None;
    }
    let rest = &path[4..];
    if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case("UNC\\") {
        return Some(format!(r"\\{}", &rest[4..]));
    }
    if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case("UNC/") {
        return Some(format!(r"\\{}", &rest[4..]));
    }
    Some(rest.to_string())
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
    found.sort_by_key(|item| std::cmp::Reverse(version_key(&item.0)));
    for (_, bin) in found {
        if !dirs.iter().any(|known| known == &bin) {
            dirs.push(bin);
        }
    }
}

fn push_fnm_bins(home: &Path, dirs: &mut Vec<PathBuf>) {
    let versions = home.join(".fnm/node-versions");
    let mut found = collect_version_bins(&versions, "installation/bin");
    found.sort_by_key(|item| std::cmp::Reverse(version_key(&item.0)));
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

#[cfg(test)]
mod tests {
    use super::{canonicalize_for_child, strip_windows_namespace, strip_windows_namespace_str};
    use std::path::PathBuf;

    #[test]
    fn strips_verbatim_disk_paths() {
        assert_eq!(
            strip_windows_namespace_str(r"\\?\C:\Users\Vince\bin.js").as_deref(),
            Some(r"C:\Users\Vince\bin.js")
        );
        assert_eq!(
            strip_windows_namespace_str(r"//?/C:/Users/Vince/bin.js").as_deref(),
            Some(r"C:/Users/Vince/bin.js")
        );
    }

    #[test]
    fn strips_verbatim_unc_paths() {
        assert_eq!(
            strip_windows_namespace_str(r"\\?\UNC\server\share\foo.js").as_deref(),
            Some(r"\\server\share\foo.js")
        );
    }

    #[test]
    fn leaves_ordinary_paths_alone() {
        assert_eq!(strip_windows_namespace_str(r"C:\Users\Vince\bin.js"), None);
        assert_eq!(strip_windows_namespace_str("/usr/bin/node"), None);
        assert_eq!(
            strip_windows_namespace(PathBuf::from(r"C:\Users\Vince\bin.js")),
            PathBuf::from(r"C:\Users\Vince\bin.js")
        );
    }

    #[test]
    fn canonicalize_for_child_does_not_keep_verbatim_prefix() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/paths.rs");
        let resolved = canonicalize_for_child(&manifest).expect("paths.rs exists");
        let text = resolved.to_string_lossy();
        assert!(
            !text.starts_with(r"\\?\"),
            "child path still has a verbatim prefix: {text}"
        );
        assert!(resolved.is_file());
    }
}
