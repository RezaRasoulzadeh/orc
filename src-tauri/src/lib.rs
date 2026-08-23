use orc::app::OrcApp;
use serde::Serialize;
use std::sync::Mutex;
use tauri::{Emitter, Manager};

struct AppState(Mutex<OrcApp>);

#[derive(Debug, Serialize)]
struct DesktopSnapshot {
    dashboard: orc::read_model::Dashboard,
    health: orc::read_model::ProjectHealth,
}

#[derive(Debug, Serialize)]
struct ManualWorkspaceInfo {
    supported: bool,
    url: Option<String>,
    error: Option<String>,
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
fn agents(state: tauri::State<'_, AppState>) -> Result<Vec<orc::registry::AgentDefinition>, String> {
    state.0.lock().map_err(|_| "application lock poisoned".to_string())?.agents().map_err(|error| error.to_string())
}

#[tauri::command]
fn configure_agent(state: tauri::State<'_, AppState>, id: String, field: String, value: String) -> Result<(), String> {
    let app = state.0.lock().map_err(|_| "application lock poisoned".to_string())?;
    let changed = match field.as_str() {
        "enabled" => app.set_agent_enabled(&id, value.parse().map_err(|_| "enabled must be true or false")?),
        "priority" => app.set_agent_priority(&id, value.parse().map_err(|_| "priority must be an integer")?),
        "profile_path" => app.set_agent_profile(&id, &value),
        "model" => app.set_agent_model(&id, &value),
        "reasoning_effort" => app.set_agent_effort(&id, orc::registry::ReasoningEffort::parse(&value).map_err(|error| error.to_string())?),
        _ => return Err(format!("unsupported agent setting: {field}")),
    }.map_err(|error| error.to_string())?;
    if changed { Ok(()) } else { Err(format!("agent '{id}' not found")) }
}

#[tauri::command]
fn sync_agent(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    state.0.lock().map_err(|_| "application lock poisoned".to_string())?.sync_agent_capacity(&id).map_err(|error| error.to_string())
}

#[tauri::command]
fn manual_runs(state: tauri::State<'_, AppState>, agent_id: String) -> Result<Vec<orc::app::ManualRunContext>, String> {
    state.0.lock().map_err(|_| "application lock poisoned".to_string())?.manual_runs(&agent_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn manual_run_action(state: tauri::State<'_, AppState>, action: String, run_id: i64, value: String) -> Result<(), String> {
    let app = state.0.lock().map_err(|_| "application lock poisoned".to_string())?;
    match action.as_str() {
        "submit" => app.submit_manual_run(run_id, &value).map(|_| ()),
        "patch" => app.submit_patch(run_id, &value).map(|_| ()),
        "fail" => app.fail_manual_run(run_id, &value).map(|_| ()),
        _ => Err(anyhow::anyhow!("unknown manual run action: {action}")),
    }.map_err(|error| error.to_string())
}

fn workspace_url(app: &OrcApp, agent_id: &str) -> Result<tauri::Url, String> {
    let agent = app.agents().map_err(|error| error.to_string())?.into_iter().find(|agent| agent.id == agent_id).ok_or_else(|| format!("agent '{agent_id}' not found"))?;
    let configured = orc::registry::manual_workspace_url(&agent).map_err(|error| error.to_string())?.ok_or_else(|| format!("manual provider '{}' has no configured workspace URL", agent.backend))?;
    let url = tauri::Url::parse(&configured).map_err(|error| format!("invalid manual workspace URL: {error}"))?;
    if url.scheme() != "https" || url.host_str().is_none() {
        return Err("manual workspace URL must be an absolute HTTPS URL".into());
    }
    Ok(url)
}

#[tauri::command]
fn manual_workspace_info(state: tauri::State<'_, AppState>, agent_id: String) -> ManualWorkspaceInfo {
    let result = state.0.lock().map_err(|_| "application lock poisoned".to_string()).and_then(|app| workspace_url(&app, &agent_id));
    match result {
        Ok(url) => ManualWorkspaceInfo { supported: true, url: Some(url.to_string()), error: None },
        Err(error) => ManualWorkspaceInfo { supported: false, url: None, error: Some(error) },
    }
}

#[tauri::command]
fn open_manual_workspace(app_handle: tauri::AppHandle, state: tauri::State<'_, AppState>, agent_id: String) -> Result<(), String> {
    let url = {
        let app = state.0.lock().map_err(|_| "application lock poisoned".to_string())?;
        workspace_url(&app, &agent_id)?
    }
    let label = format!("manual-{}", agent_id.chars().map(|character| if character.is_ascii_alphanumeric() { character } else { '-' }).collect::<String>());
    if let Some(window) = app_handle.get_webview_window(&label) {
        window.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }
    let scheme = url.scheme().to_string();
    let host = url.host_str().map(str::to_owned);
    let port = url.port_or_known_default();
    tauri::WebviewWindowBuilder::new(&app_handle, label, tauri::WebviewUrl::External(url))
        .title(format!("Orc Manual Workspace · {agent_id}"))
        .inner_size(1120.0, 800.0)
        .on_navigation(move |candidate| candidate.scheme() == scheme && candidate.host_str() == host.as_deref() && candidate.port_or_known_default() == port)
        .build()
        .map_err(|error| format!("provider could not render in an embedded webview: {error}"))?;
    Ok(())
}

#[tauri::command]
fn close_manual_workspace(app_handle: tauri::AppHandle, agent_id: String) -> Result<(), String> {
    let label = format!("manual-{}", agent_id.chars().map(|character| if character.is_ascii_alphanumeric() { character } else { '-' }).collect::<String>());
    if let Some(window) = app_handle.get_webview_window(&label) {
        window.close().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn queue(state: tauri::State<'_, AppState>) -> Result<orc::queue::QueueReport, String> {
    state.0.lock().map_err(|_| "application lock poisoned".to_string())?.queue().map_err(|error| error.to_string())
}

#[tauri::command]
fn planning_request(state: tauri::State<'_, AppState>) -> Result<orc::protocol::PlanningRequest, String> { state.0.lock().map_err(|_| "application lock poisoned".to_string())?.planning_request().map_err(|error| error.to_string()) }
#[tauri::command]
fn planner_validate(state: tauri::State<'_, AppState>, json: String) -> Result<orc::protocol::PlanResponse, String> { state.0.lock().map_err(|_| "application lock poisoned".to_string())?.validate_plan_json(&json).map_err(|error| error.to_string()) }
#[tauri::command]
fn planner_apply(state: tauri::State<'_, AppState>, json: String) -> Result<std::collections::BTreeMap<String, String>, String> { let app = state.0.lock().map_err(|_| "application lock poisoned".to_string())?; let plan = app.validate_plan_json(&json).map_err(|error| error.to_string())?; app.apply_plan(&plan).map_err(|error| error.to_string()) }
#[tauri::command]
fn approvals(state: tauri::State<'_, AppState>) -> Result<Vec<orc::storage::db::ApprovalRequest>, String> { state.0.lock().map_err(|_| "application lock poisoned".to_string())?.approvals().map_err(|error| error.to_string()) }
#[tauri::command]
fn resolve_approval(state: tauri::State<'_, AppState>, id: i64) -> Result<(), String> { state.0.lock().map_err(|_| "application lock poisoned".to_string())?.resolve_approval(id).map_err(|error| error.to_string()) }
#[tauri::command]
fn project_report(state: tauri::State<'_, AppState>) -> Result<orc::protocol::ProjectReport, String> { state.0.lock().map_err(|_| "application lock poisoned".to_string())?.project_report().map_err(|error| error.to_string()) }

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
        "cancel" => app.cancel(&task_id, reason.as_deref()).map_err(anyhow::Error::from),
        "requeue" => app.requeue(&task_id),
        "add_dependency" => app.add_dependency(&task_id, reason.as_deref().ok_or_else(|| "dependency id is required".to_string())?),
        "remove_dependency" => app.remove_dependency(&task_id, reason.as_deref().ok_or_else(|| "dependency id is required".to_string())?).map(|_| ()),
        "revise" => app.revise(&task_id, reason.as_deref().ok_or_else(|| "feedback is required".to_string())?, agent_id.as_deref().ok_or_else(|| "agent id is required".to_string())?),
        _ => Err(anyhow::anyhow!("unknown task action: {action}")),
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
    let mut root = std::env::current_dir()?;
    if root.file_name().is_some_and(|name| name == "src-tauri") {
        root.pop();
    }
    let app = OrcApp::open(root.join(".orc/orc.db"), &root)?;
    tauri::Builder::default()
        .manage(AppState(Mutex::new(app)))
        .setup(|handle| {
            let state = handle.state::<AppState>();
            let subscription = state.0.lock().map_err(|_| anyhow::anyhow!("application lock poisoned"))?.subscribe();
            let handle = handle.handle().clone();
            std::thread::spawn(move || {
                while let Ok(event) = subscription.recv() {
                    if handle.emit("orc://run-event", event).is_err() { break; }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![snapshot, tasks, agents, configure_agent, sync_agent, manual_runs, manual_run_action, manual_workspace_info, open_manual_workspace, close_manual_workspace, queue, planning_request, planner_validate, planner_apply, approvals, resolve_approval, project_report, task_details, review, dispatch, task_action, runs, runs_workspace, run_details, lead_context, lead_proposals, invoke_lead, apply_lead_proposal, reject_lead_proposal])
        .run(tauri::generate_context!())?;
    Ok(())
}
