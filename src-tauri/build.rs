/// Commands oardsh exposes over IPC. Listing them here autogenerates the
/// `allow-*` permissions the capability files grant per origin; without them
/// Tauri rejects every app command from the remote dsh pages. Keep in sync
/// with `generate_handler!` in `src/lib.rs`.
const COMMANDS: &[&str] = &[
    "dsh_status",
    "restart_dsh",
    "proxy_config",
    "set_proxy_config",
    "token_usage",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
}
