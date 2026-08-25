use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, WebviewWindow};

const STATE_FILE: &str = "window-state.json";
const SAVE_DEBOUNCE: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub width: f64,
    pub height: f64,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub maximized: bool,
}

impl Default for WindowGeometry {
    fn default() -> Self {
        Self {
            width: 1280.0,
            height: 800.0,
            x: None,
            y: None,
            maximized: false,
        }
    }
}

pub struct WindowStateStore {
    path: PathBuf,
    dirty_since: Option<Instant>,
}

impl WindowStateStore {
    pub fn new(app: &AppHandle) -> Result<Self, String> {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|err| format!("failed to resolve app data dir: {err}"))?;
        fs::create_dir_all(&dir).map_err(|err| format!("failed to create app data dir: {err}"))?;
        Ok(Self {
            path: dir.join(STATE_FILE),
            dirty_since: None,
        })
    }

    pub fn load(&self) -> Option<WindowGeometry> {
        let raw = fs::read_to_string(&self.path).ok()?;
        match serde_json::from_str::<WindowGeometry>(&raw) {
            Ok(geometry) if geometry.width > 0.0 && geometry.height > 0.0 => Some(geometry),
            _ => None,
        }
    }

    pub fn note_changed(&mut self) {
        if self.dirty_since.is_none() {
            self.dirty_since = Some(Instant::now());
        }
    }

    pub fn flush_if_due(&mut self, app: &AppHandle, force: bool) {
        let due = match self.dirty_since {
            Some(at) => force || at.elapsed() >= SAVE_DEBOUNCE,
            None => false,
        };
        if due {
            self.save_now(app);
        }
    }

    pub fn save_now(&mut self, app: &AppHandle) {
        self.dirty_since = None;
        for (_, window) in app.webview_windows() {
            if write_window_geometry(&window, &self.path).is_ok() {
                return;
            }
        }
    }
}

fn write_window_geometry(window: &WebviewWindow, path: &PathBuf) -> Result<(), String> {
    let scale = window.scale_factor().unwrap_or(1.0);
    let size = window.inner_size().map_err(|err| err.to_string())?;
    let position = window.outer_position().ok();
    let geometry = WindowGeometry {
        width: size.to_logical(scale).width,
        height: size.to_logical(scale).height,
        x: position.map(|p| p.to_logical(scale).x),
        y: position.map(|p| p.to_logical(scale).y),
        maximized: window.is_maximized().unwrap_or(false),
    };
    let serialized = serde_json::to_string(&geometry)
        .map_err(|err| format!("failed to serialize state: {err}"))?;
    fs::write(path, serialized).map_err(|err| format!("failed to persist state: {err}"))
}

pub fn apply_saved_geometry(window: &WebviewWindow, geometry: &WindowGeometry) {
    let _ = window.set_size(tauri::LogicalSize::new(geometry.width, geometry.height));
    if let (Some(x), Some(y)) = (geometry.x, geometry.y) {
        let _ = window.set_position(tauri::LogicalPosition::new(x, y));
    }
    if geometry.maximized {
        let _ = window.maximize();
    }
}
