// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod deep_link;
mod host_lifecycle;
mod host_status;
mod transport;
mod window_state;

use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{Emitter, Manager, RunEvent, WindowEvent};
use window_state::WindowStateStore;

const MAIN_WINDOW: &str = "main";
pub const NAVIGATE_EVENT: &str = "shell://navigate";

fn build_menu(app: &tauri::AppHandle) -> tauri::Result<()> {
    let file_menu = SubmenuBuilder::new(app, "File")
        .item(&MenuItemBuilder::with_id("open-host-status", "Host Status").build(app)?)
        .separator()
        .close_window()
        .item(&MenuItemBuilder::with_id("quit", "Quit Lazarus").build(app)?)
        .build()?;
    let edit_menu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let view_menu = SubmenuBuilder::new(app, "View")
        .item(&MenuItemBuilder::with_id("reload", "Reload").build(app)?)
        .build()?;
    let help_menu = SubmenuBuilder::new(app, "Help")
        .item(&MenuItemBuilder::with_id("about", "About Lazarus").build(app)?)
        .build()?;
    let menu = MenuBuilder::new(app)
        .items(&[&file_menu, &edit_menu, &view_menu, &help_menu])
        .build()?;
    app.set_menu(menu)?;
    Ok(())
}

fn focus_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            focus_main_window(app);
            deep_link::emit_from_argv(app, &argv);
        }))
        .invoke_handler(tauri::generate_handler![
            host_status::host_status,
            host_lifecycle::host_start,
            host_lifecycle::host_stop,
            host_lifecycle::host_doctor,
            host_lifecycle::host_ensure,
            host_lifecycle::host_update,
            host_lifecycle::host_rollback,
            transport::host_ipc_request,
            transport::host_ipc_cancel,
            transport::host_ipc_open_events,
        ])
        .setup(|app| {
            build_menu(app.handle())?;
            let mut store = WindowStateStore::new(app.handle())?;
            if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
                if let Some(geometry) = store.load() {
                    window_state::apply_saved_geometry(&window, &geometry);
                }
                store.save_now(app.handle());
                deep_link::emit_from_argv(
                    app.handle(),
                    &std::env::args().skip(1).collect::<Vec<String>>(),
                );
            }
            app.manage(std::sync::Mutex::new(store));
            Ok(())
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "quit" => app.exit(0),
            "open-host-status" => {
                focus_main_window(app);
                if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
                    let _ = window.emit(NAVIGATE_EVENT, "/host-status");
                }
            }
            "reload" => {
                if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
                    let _ = window.eval("window.location.reload()");
                }
            }
            "about" => {
                if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
                    let _ = window.emit(NAVIGATE_EVENT, "/");
                }
            }
            _ => {}
        })
        .on_window_event(|window, event| match event {
            WindowEvent::Resized(_) | WindowEvent::Moved(_) => {
                let app = window.app_handle();
                if let Some(store) = app.try_state::<std::sync::Mutex<WindowStateStore>>() {
                    let mut guard = store
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    guard.note_changed();
                    guard.flush_if_due(app, false);
                }
            }
            WindowEvent::CloseRequested { .. } => {
                let app = window.app_handle();
                if let Some(store) = app.try_state::<std::sync::Mutex<WindowStateStore>>() {
                    let mut guard = store
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    guard.save_now(app);
                }
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("failed to build the Lazarus desktop shell");

    app.run(|app, event| {
        if let RunEvent::ExitRequested { .. } = event {
            flush_window_state(app);
        }
    });
}

fn flush_window_state(app: &tauri::AppHandle) {
    let Some(store) = app.try_state::<std::sync::Mutex<WindowStateStore>>() else {
        return;
    };
    let mut guard = store
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.flush_if_due(app, true);
    guard.save_now(app);
}
