use orc::app::OrcApp;
use serde::Serialize;
use std::sync::Mutex;
use tauri::Emitter;

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
fn queue(state: tauri::State<'_, AppState>) -> Result<orc::queue::QueueReport, String> {
    state.0.lock().map_err(|_| "application lock poisoned".to_string())?.queue().map_err(|error| error.to_string())
}

#[tauri::command]
fn task_details(state: tauri::State<'_, AppState>, task_id: String, activity_limit: usize) -> Result<Option<orc::read_model::TaskDetails>, String> {
    state.0.lock().map_err(|_| "application lock poisoned".to_string())?.task_details(&task_id, activity_limit).map_err(|error| error.to_string())
}

#[tauri::command]
fn review(state: tauri::State<'_, AppState>, task_id: String) -> Result<orc::review::ReviewSummary, String> {
    state.0.lock().map_err(|_| "application lock poisoned".to_string())?.review(&task_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn dispatch(state: tauri::State<'_, AppState>, task_id: String, agent_id: Option<String>) -> Result<orc::review::DispatchSummary, String> {
    state.0.lock().map_err(|_| "application lock poisoned".to_string())?.dispatch(&task_id, agent_id.as_deref()).map_err(|error| error.to_string())
}

#[tauri::command]
fn task_action(state: tauri::State<'_, AppState>, action: String, task_id: String, reason: Option<String>, agent_id: Option<String>) -> Result<(), String> {
    let app = state.0.lock().map_err(|_| "application lock poisoned".to_string())?;
    match action.as_str() {
        "dispatch" => app.dispatch(&task_id, agent_id.as_deref()).map(|_| ()),
        "accept" => app.accept(&task_id),
        "reject" => app.reject(&task_id, reason.as_deref()),
        "cancel" => app.cancel(&task_id, reason.as_deref()).map_err(|error| error.to_string()),
        "requeue" => app.requeue(&task_id),
        "add_dependency" => app.add_dependency(&task_id, reason.as_deref().ok_or_else(|| "dependency id is required".to_string())?),
        "remove_dependency" => app.remove_dependency(&task_id, reason.as_deref().ok_or_else(|| "dependency id is required".to_string())?).map(|_| ()),
        "revise" => app.revise(&task_id, reason.as_deref().ok_or_else(|| "feedback is required".to_string())?, agent_id.as_deref().ok_or_else(|| "agent id is required".to_string())?),
        _ => Err(format!("unknown task action: {action}")),
    }.map_err(|error| error.to_string())
}

#[tauri::command]
fn runs(state: tauri::State<'_, AppState>, limit: usize) -> Result<Vec<orc::storage::AgentRun>, String> {
    state.0.lock().map_err(|_| "application lock poisoned".to_string())?.runs(limit).map_err(|error| error.to_string())
}

#[tauri::command]
fn runs_workspace(state: tauri::State<'_, AppState>, limit: usize, activity_limit: usize) -> Result<orc::read_model::RunsWorkspace, String> {
    state.0.lock().map_err(|_| "application lock poisoned".to_string())?.runs_workspace(limit, activity_limit).map_err(|error| error.to_string())
}

#[tauri::command]
fn run_details(state: tauri::State<'_, AppState>, run_id: i64, activity_limit: usize) -> Result<Option<orc::read_model::RunDetails>, String> {
    state.0.lock().map_err(|_| "application lock poisoned".to_string())?.run_details(run_id, activity_limit).map_err(|error| error.to_string())
}

#[tauri::command]
fn lead_context(state: tauri::State<'_, AppState>, limit: usize) -> Result<orc::lead::LeadContext, String> {
    state.0.lock().map_err(|_| "application lock poisoned".to_string())?.lead().context(limit).map_err(|error| error.to_string())
}

#[tauri::command]
fn lead_proposals(state: tauri::State<'_, AppState>) -> Result<Vec<orc::lead::LeadProposal>, String> {
    state.0.lock().map_err(|_| "application lock poisoned".to_string())?.lead().pending_proposals().map_err(|error| error.to_string())
}

#[tauri::command]
fn invoke_lead(
    state: tauri::State<'_, AppState>,
    message: String,
    config: Option<orc::lead::LeadProviderConfig>,
) -> Result<orc::lead::LeadResponse, String> {
    let config = config.unwrap_or_else(|| orc::lead::LeadProviderConfig {
        agent_id: "project-lead".into(),
        model: None,
        reasoning_effort: None,
    });
    state
        .0
        .lock()
        .map_err(|_| "application lock poisoned".to_string())?
        .invoke_configured_lead(&message, &config, 20)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn apply_lead_proposal(state: tauri::State<'_, AppState>, proposal_id: i64) -> Result<(), String> {
    state.0.lock().map_err(|_| "application lock poisoned".to_string())?.apply_lead_proposal(proposal_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn reject_lead_proposal(state: tauri::State<'_, AppState>, proposal_id: i64) -> Result<(), String> {
    let changed = state.0.lock().map_err(|_| "application lock poisoned".to_string())?.lead().reject_proposal(proposal_id).map_err(|error| error.to_string())?;
    if changed { Ok(()) } else { Err("Lead proposal is no longer pending".into()) }
}

pub fn run() -> anyhow::Result<()> {
    let root = std::env::current_dir()?;
    let app = OrcApp::open(root.join(".orc/orc.db"), &root)?;
    tauri::Builder::default()
        .manage(AppState(Mutex::new(app)))
        .setup(|handle| {
            let state = handle.state::<AppState>();
            let subscription = state.0.lock().map_err(|_| anyhow::anyhow!("application lock poisoned"))?.subscribe();
            let handle = handle.clone();
            std::thread::spawn(move || {
                while let Ok(event) = subscription.recv() {
                    if handle.emit("orc://run-event", event).is_err() { break; }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![snapshot, tasks, queue, task_details, review, dispatch, task_action, runs, runs_workspace, run_details, lead_context, lead_proposals, invoke_lead, apply_lead_proposal, reject_lead_proposal])
        .run(tauri::generate_context!())?;
    Ok(())
}
