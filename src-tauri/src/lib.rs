mod engine;
mod i18n;
mod paths;
mod usage;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, RunEvent};
use tauri_plugin_opener::OpenerExt;

use engine::Engine;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_notification::init())
        .manage(Engine::new())
        .invoke_handler(tauri::generate_handler![
            engine::dsh_status,
            engine::native_web_event,
            usage::token_usage,
        ])
        .setup(|app| {
            let menu = build_menu(app.handle())?;
            app.set_menu(menu)?;
            engine::remember_boot_url(app.handle());
            engine::boot_dsh(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            // Otherwise the dsh process outlives the window with its port bound.
            // The window is already going away, so do not navigate it.
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                let app = window.app_handle();
                app.state::<Engine>().stop(app, false);
            }
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "restart" => engine::restart_from_menu(app),
            "reload" => engine::reload_main(app),
            "docs" => {
                let _ = app.opener().open_url(
                    "https://github.com/deepseek-ai/deepseek-harness",
                    None::<&str>,
                );
            }
            "web-guide" => {
                let _ = app.opener().open_url(
                    "https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/user/guide/index.md",
                    None::<&str>,
                );
            }
            #[cfg(debug_assertions)]
            "devtools" => {
                if let Some(window) = engine::current_window(app) {
                    window.open_devtools();
                }
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if matches!(event, RunEvent::Exit | RunEvent::ExitRequested { .. }) {
                app.state::<Engine>().stop(app, false);
            }
        });
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let pkg = app.package_info();
    let about = tauri::menu::AboutMetadata {
        name: Some(pkg.name.clone()),
        version: Some(pkg.version.to_string()),
        copyright: Some("Desktop shell for DeepSeek Harness".into()),
        ..Default::default()
    };

    let restart = MenuItem::with_id(
        app,
        "restart",
        "Restart Server",
        true,
        Some("CmdOrCtrl+Shift+R"),
    )?;
    let reload = MenuItem::with_id(app, "reload", "Reload", true, Some("CmdOrCtrl+R"))?;
    let docs = MenuItem::with_id(
        app,
        "docs",
        "DeepSeek Harness on GitHub",
        true,
        None::<&str>,
    )?;
    let guide = MenuItem::with_id(app, "web-guide", "Web UI Guide", true, None::<&str>)?;

    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &restart,
            #[cfg(not(target_os = "macos"))]
            &PredefinedMenuItem::separator(app)?,
            #[cfg(not(target_os = "macos"))]
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    let view = {
        #[cfg(all(target_os = "macos", debug_assertions))]
        {
            let fullscreen = PredefinedMenuItem::fullscreen(app, None)?;
            let devtools = MenuItem::with_id(
                app,
                "devtools",
                "Toggle Developer Tools",
                true,
                Some("Alt+Cmd+I"),
            )?;
            Submenu::with_items(app, "View", true, &[&reload, &fullscreen, &devtools])?
        }
        #[cfg(all(target_os = "macos", not(debug_assertions)))]
        {
            let fullscreen = PredefinedMenuItem::fullscreen(app, None)?;
            Submenu::with_items(app, "View", true, &[&reload, &fullscreen])?
        }
        #[cfg(all(not(target_os = "macos"), debug_assertions))]
        {
            let devtools =
                MenuItem::with_id(app, "devtools", "Toggle Developer Tools", true, Some("F12"))?;
            Submenu::with_items(app, "View", true, &[&reload, &devtools])?
        }
        #[cfg(all(not(target_os = "macos"), not(debug_assertions)))]
        {
            Submenu::with_items(app, "View", true, &[&reload])?
        }
    };

    let window = Submenu::with_id_and_items(
        app,
        tauri::menu::WINDOW_SUBMENU_ID,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;

    let help = Submenu::with_id_and_items(
        app,
        tauri::menu::HELP_SUBMENU_ID,
        "Help",
        true,
        &[&docs, &guide],
    )?;

    #[cfg(target_os = "macos")]
    let app_menu = Submenu::with_items(
        app,
        pkg.name.clone(),
        true,
        &[
            &PredefinedMenuItem::about(app, None, Some(about))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    #[cfg(target_os = "macos")]
    {
        Menu::with_items(app, &[&app_menu, &file, &edit, &view, &window, &help])
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = about;
        Menu::with_items(app, &[&file, &edit, &view, &window, &help])
    }
}

#[cfg(test)]
mod tests {
    /// `generate_handler!`, `COMMANDS` in build.rs and the capability grants
    /// must name the same commands. Drift is silent at build time and only
    /// surfaces at runtime as "not allowed. Plugin not found".
    #[test]
    fn acl_manifest_covers_every_registered_command() {
        let lib = include_str!("lib.rs");
        let handler = lib
            .split_once("generate_handler![")
            .and_then(|(_, rest)| rest.split_once(']'))
            .expect("generate_handler! block")
            .0;
        let registered: Vec<&str> = handler
            .split(',')
            .filter_map(|entry| entry.trim().rsplit("::").next())
            .filter(|entry| !entry.is_empty())
            .collect();
        assert!(registered.len() > 2, "parsed too few commands");

        let build = include_str!("../build.rs");
        let declared = build
            .split_once("const COMMANDS: &[&str] = &[")
            .and_then(|(_, rest)| rest.split_once("];"))
            .expect("COMMANDS list")
            .0;
        for command in &registered {
            assert!(
                declared.contains(&format!("\"{command}\"")),
                "{command} is registered but missing from COMMANDS in build.rs"
            );
        }

        for capability in [
            include_str!("../capabilities/default.json"),
            include_str!("../capabilities/dsh-bridge.json"),
        ] {
            for granted in capability
                .split('"')
                .filter_map(|token| token.strip_prefix("allow-"))
            {
                let command = granted.replace('-', "_");
                assert!(
                    registered.contains(&command.as_str()),
                    "capability grants allow-{granted}, which is not a registered command"
                );
            }
        }
    }
}
