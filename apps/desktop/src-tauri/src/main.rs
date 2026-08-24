// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod host_lifecycle;
mod host_status;

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            host_status::host_status,
            host_lifecycle::host_start,
            host_lifecycle::host_stop,
            host_lifecycle::host_doctor,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run the Lazarus desktop shell");
}
