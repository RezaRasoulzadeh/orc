use orc::app::OrcApp;
use serde::Serialize;
use std::sync::Mutex;

struct AppState(Mutex<OrcApp>);

#[derive(Debug, Serialize)]
struct DesktopSnapshot {
    dashboard: orc::read_model::Dashboard,
    health: orc::read_model::ProjectHealth,
}

#[tauri::command]
fn snapshot(state: tauri::State<'_, AppState>) -> Result<DesktopSnapshot, String> {
    let app = state.0.lock().map_err(|_| "application lock poisoned".to_string())?;
    Ok(DesktopSnapshot {
        dashboard: app.dashboard(24).map_err(|error| error.to_string())?,
        health: app.project_health().map_err(|error| error.to_string())?,
    })
}

#[tauri::command]
fn tasks(state: tauri::State<'_, AppState>) -> Result<Vec<orc::task::Task>, String> {
    state.0.lock().map_err(|_| "application lock poisoned".to_string())?.tasks().map_err(|error| error.to_string())
}

#[tauri::command]
fn runs(state: tauri::State<'_, AppState>, limit: usize) -> Result<Vec<orc::storage::AgentRun>, String> {
    state.0.lock().map_err(|_| "application lock poisoned".to_string())?.runs(limit).map_err(|error| error.to_string())
}

pub fn run() -> anyhow::Result<()> {
    let root = std::env::current_dir()?;
    let app = OrcApp::open(root.join(".orc/orc.db"), &root)?;
    tauri::Builder::default()
        .manage(AppState(Mutex::new(app)))
        .invoke_handler(tauri::generate_handler![snapshot, tasks, runs])
        .run(tauri::generate_context!())?;
    Ok(())
}
