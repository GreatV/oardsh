mod download;
mod engine;
mod i18n;
mod paths;
mod proxy;
mod ready;
mod sidecar;
mod usage;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_window_state::StateFlags;

use engine::Engine;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            engine::reveal_main(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::all().difference(StateFlags::VISIBLE))
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .manage(Engine::new())
        .invoke_handler(tauri::generate_handler![
            engine::dsh_status,
            engine::restart_dsh,
            proxy::proxy_config,
            proxy::set_proxy_config,
            usage::token_usage,
        ])
        .setup(|app| {
            let menu = build_menu(app.handle())?;
            app.set_menu(menu)?;
            build_tray(app.handle())?;
            build_main_window(app.handle())?;
            engine::remember_boot_url(app.handle());
            engine::boot_dsh(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                engine::note_hidden_to_tray(window.app_handle());
            }
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "restart" => engine::restart_from_menu(app),
            "quit" => engine::quit_app(app),
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
        .run(|app, event| match event {
            RunEvent::ExitRequested { api, code, .. } => {
                // None: the last window was "closed". We hid it to the tray.
                if code.is_none() {
                    api.prevent_exit();
                    return;
                }
                app.state::<Engine>().stop(app, false);
            }
            RunEvent::Exit => {
                app.state::<Engine>().stop(app, false);
            }
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => engine::reveal_main(app),
            _ => {}
        });
}

/// The main window is built here instead of tauri.conf.json because a download
/// handler can only be attached in code, and the dsh page depends on it for its
/// session-log export. Keep the geometry in sync with what the config carried.
fn build_main_window(app: &AppHandle) -> tauri::Result<()> {
    tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::App("index.html".into()))
        .title("DeepSeek Harness")
        .inner_size(1440.0, 900.0)
        .min_inner_size(960.0, 640.0)
        .center()
        .on_download(download::handle)
        .build()?;
    Ok(())
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let locale = i18n::system_locale();
    let show = MenuItem::with_id(
        app,
        "tray-show",
        i18n::translate(locale, "tray.show"),
        true,
        None::<&str>,
    )?;
    let restart = MenuItem::with_id(
        app,
        "tray-restart",
        i18n::translate(locale, "tray.restart"),
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(
        app,
        "tray-quit",
        i18n::translate(locale, "tray.quit"),
        true,
        None::<&str>,
    )?;
    let menu = Menu::with_items(app, &[&show, &restart, &quit])?;
    let icon = tray_icon();
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .menu(&menu)
        .tooltip("oardsh")
        .icon_as_template(cfg!(target_os = "macos"))
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                engine::reveal_main(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "tray-show" => engine::reveal_main(app),
            "tray-restart" => engine::restart_from_menu(app),
            "tray-quit" => engine::quit_app(app),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

/// The app icon's drawing with its plate taken away, so the menu bar paints it
/// in its own ink instead of showing a white tile with a speck on it.
fn tray_icon() -> tauri::image::Image<'static> {
    // macOS paints template images from alpha, so the glyph ships as a mask.
    // Everywhere else the taskbar shows the icon as-is, over a background that
    // is light in one theme and dark in the other, so it keeps its plate.
    let bytes: &'static [u8] = if cfg!(target_os = "macos") {
        include_bytes!("../icons/tray-mac.png")
    } else {
        include_bytes!("../icons/tray.png")
    };
    tauri::image::Image::from_bytes(bytes).expect("the tray glyph is a valid PNG")
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let pkg = app.package_info();
    let about = tauri::menu::AboutMetadata {
        name: Some(pkg.name.clone()),
        version: Some(pkg.version.to_string()),
        copyright: Some("Desktop shell for DeepSeek Harness".into()),
        ..Default::default()
    };

    let locale = i18n::system_locale();
    let restart = MenuItem::with_id(
        app,
        "restart",
        i18n::translate(locale, "menu.restart"),
        true,
        Some("CmdOrCtrl+Shift+R"),
    )?;
    // Custom quit so Cmd+Q / the menu item stop dsh. The predefined Quit
    // item is treated as "user closed the last window" and would be swallowed
    // by the hide-to-tray ExitRequested handler.
    let quit = MenuItem::with_id(
        app,
        "quit",
        i18n::translate(locale, "menu.quit"),
        true,
        Some("CmdOrCtrl+Q"),
    )?;
    let reload = MenuItem::with_id(
        app,
        "reload",
        i18n::translate(locale, "menu.reload"),
        true,
        Some("CmdOrCtrl+R"),
    )?;
    let docs = MenuItem::with_id(
        app,
        "docs",
        i18n::translate(locale, "menu.docs"),
        true,
        None::<&str>,
    )?;
    let guide = MenuItem::with_id(
        app,
        "web-guide",
        i18n::translate(locale, "menu.guide"),
        true,
        None::<&str>,
    )?;

    let file = Submenu::with_items(
        app,
        i18n::translate(locale, "menu.file"),
        true,
        &[
            &restart,
            #[cfg(not(target_os = "macos"))]
            &PredefinedMenuItem::separator(app)?,
            #[cfg(not(target_os = "macos"))]
            &quit,
        ],
    )?;

    let edit = Submenu::with_items(
        app,
        i18n::translate(locale, "menu.edit"),
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
                i18n::translate(locale, "menu.devtools"),
                true,
                Some("Alt+Cmd+I"),
            )?;
            Submenu::with_items(
                app,
                i18n::translate(locale, "menu.view"),
                true,
                &[&reload, &fullscreen, &devtools],
            )?
        }
        #[cfg(all(target_os = "macos", not(debug_assertions)))]
        {
            let fullscreen = PredefinedMenuItem::fullscreen(app, None)?;
            Submenu::with_items(
                app,
                i18n::translate(locale, "menu.view"),
                true,
                &[&reload, &fullscreen],
            )?
        }
        #[cfg(all(not(target_os = "macos"), debug_assertions))]
        {
            let devtools = MenuItem::with_id(
                app,
                "devtools",
                i18n::translate(locale, "menu.devtools"),
                true,
                Some("F12"),
            )?;
            Submenu::with_items(
                app,
                i18n::translate(locale, "menu.view"),
                true,
                &[&reload, &devtools],
            )?
        }
        #[cfg(all(not(target_os = "macos"), not(debug_assertions)))]
        {
            Submenu::with_items(app, i18n::translate(locale, "menu.view"), true, &[&reload])?
        }
    };

    let window = Submenu::with_id_and_items(
        app,
        tauri::menu::WINDOW_SUBMENU_ID,
        i18n::translate(locale, "menu.window"),
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
        i18n::translate(locale, "menu.help"),
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
            &quit,
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
