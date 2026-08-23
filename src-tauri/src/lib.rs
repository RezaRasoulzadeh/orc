use anyhow::Context;
use orc::app::OrcApp;
use serde::Serialize;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tauri::{Emitter, Manager};
mod project;
mod storage;

struct AppState(SessionState);
struct RegistryState(Mutex<project::ProjectRegistry>);

struct SessionState(Mutex<Option<project::ProjectSession>>);

fn remove_project_state(
    registry: &mut project::ProjectRegistry,
    session: &mut Option<project::ProjectSession>,
    id: &str,
) -> anyhow::Result<bool> {
    let removed = registry.remove(id)?;
    if removed
        && session
            .as_ref()
            .is_some_and(|active| active.project.id == id)
    {
        *session = None;
    }
    Ok(removed)
}

struct SessionGuard<'a> {
    guard: std::sync::MutexGuard<'a, Option<project::ProjectSession>>,
    app: *const OrcApp,
}

impl SessionGuard<'_> {
    fn app(&self) -> Result<&OrcApp, String> {
        Ok(unsafe { &*self.app })
    }
}

impl SessionState {
    fn active(&self) -> Result<SessionGuard<'_>, String> {
        let guard = self
            .0
            .lock()
            .map_err(|_| "application lock poisoned".to_string())?;
        let app = guard
            .as_ref()
            .map(|session| &session.app as *const OrcApp)
            .ok_or_else(|| "no active project".to_string())?;
        Ok(SessionGuard { guard, app })
    }

    fn replace(&self, session: Option<project::ProjectSession>) -> Result<(), String> {
        *self
            .0
            .lock()
            .map_err(|_| "application lock poisoned".to_string())? = session;
        Ok(())
    }
}

#[tauri::command]
fn registered_projects(
    state: tauri::State<'_, RegistryState>,
) -> Result<Vec<project::RegisteredProject>, String> {
    state
        .0
        .lock()
        .map_err(|_| "project registry lock poisoned".to_string())
        .map(|registry| registry.projects())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn register_project(
    state: tauri::State<'_, RegistryState>,
    root: String,
    display_name: Option<String>,
) -> Result<project::RegisteredProject, String> {
    state
        .0
        .lock()
        .map_err(|_| "project registry lock poisoned".to_string())?
        .register(root, display_name)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn import_project(
    state: tauri::State<'_, RegistryState>,
    root: String,
    display_name: Option<String>,
) -> Result<project::RegisteredProject, String> {
    register_project(state, root, display_name)
}

#[tauri::command]
fn adopt_project(state: tauri::State<'_, RegistryState>, root: String, display_name: Option<String>) -> Result<project::RegisteredProject, String> {
    let adopted_root = orc::adoption::adopt(&root).map_err(|error| error.to_string())?;
    register_project(state, adopted_root.to_string_lossy().into_owned(), display_name)
}

#[tauri::command]
fn project_availability(
    state: tauri::State<'_, RegistryState>,
    id: String,
) -> Result<project::ProjectAvailability, String> {
    state
        .0
        .lock()
        .map_err(|_| "project registry lock poisoned".to_string())?
        .availability(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn relocate_project(
    state: tauri::State<'_, RegistryState>,
    id: String,
    root: String,
) -> Result<project::RegisteredProject, String> {
    state
        .0
        .lock()
        .map_err(|_| "project registry lock poisoned".to_string())?
        .relocate(&id, root)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn current_project(
    state: tauri::State<'_, AppState>,
) -> Result<Option<project::RegisteredProject>, String> {
    let guard = state
        .0
        .0
        .lock()
        .map_err(|_| "application lock poisoned".to_string())?;
    Ok(guard.as_ref().map(|session| session.project.clone()))
}

#[tauri::command]
fn remove_project(
    app_state: tauri::State<'_, AppState>,
    registry_state: tauri::State<'_, RegistryState>,
    id: String,
) -> Result<bool, String> {
    let mut registry = registry_state
        .0
        .lock()
        .map_err(|_| "project registry lock poisoned".to_string())?;
    let mut session = app_state
        .0
        .0
        .lock()
        .map_err(|_| "application lock poisoned".to_string())?;
    remove_project_state(&mut registry, &mut session, &id).map_err(|error| error.to_string())
}

#[tauri::command]
fn open_project(
    app_handle: tauri::AppHandle,
    app_state: tauri::State<'_, AppState>,
    registry_state: tauri::State<'_, RegistryState>,
    id: String,
) -> Result<(), String> {
    let project = registry_state
        .0
        .lock()
        .map_err(|_| "project registry lock poisoned".to_string())?
        .mark_opened(&id)
        .map_err(|error| error.to_string())?;
    let mut session = project::ProjectSession::open(project).map_err(|error| error.to_string())?;
    let project_id = session.project.id.clone();
    let (subscription, cancellation) = session
        .take_subscription()
        .map_err(|error| error.to_string())?;
    app_state
        .0
        .replace(Some(session))
        .map(|_| spawn_event_forwarder(app_handle, project_id, subscription, cancellation))
}

#[tauri::command]
fn close_project(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.0.replace(None)
}

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
    let guard = state.0.active()?;
    let app = guard.app()?;
    Ok(DesktopSnapshot {
        dashboard: app.dashboard(24).map_err(|error| error.to_string())?,
        health: app.project_health().map_err(|error| error.to_string())?,
    })
}

#[tauri::command]
fn tasks(state: tauri::State<'_, AppState>) -> Result<Vec<orc::task::Task>, String> {
    state
        .0
        .active()?
        .app()?
        .tasks()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn agents(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<orc::registry::AgentDefinition>, String> {
    state
        .0
        .active()?
        .app()?
        .agents()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn configure_agent(
    state: tauri::State<'_, AppState>,
    id: String,
    field: String,
    value: String,
) -> Result<(), String> {
    let guard = state.0.active()?;
    let app = guard.app()?;
    let changed = match field.as_str() {
        "enabled" => app.set_agent_enabled(
            &id,
            value.parse().map_err(|_| "enabled must be true or false")?,
        ),
        "priority" => app.set_agent_priority(
            &id,
            value.parse().map_err(|_| "priority must be an integer")?,
        ),
        "profile_path" => app.set_agent_profile(&id, &value),
        "model" => app.set_agent_model(&id, &value),
        "reasoning_effort" => app.set_agent_effort(
            &id,
            orc::registry::ReasoningEffort::parse(&value).map_err(|error| error.to_string())?,
        ),
        _ => return Err(format!("unsupported agent setting: {field}")),
    }
    .map_err(|error| error.to_string())?;
    if changed {
        Ok(())
    } else {
        Err(format!("agent '{id}' not found"))
    }
}

#[tauri::command]
fn sync_agent(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .0
        .active()?
        .app()?
        .sync_agent_capacity(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn manual_runs(
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> Result<Vec<orc::app::ManualRunContext>, String> {
    state
        .0
        .active()?
        .app()?
        .manual_runs(&agent_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn manual_run_action(
    state: tauri::State<'_, AppState>,
    action: String,
    run_id: i64,
    value: String,
) -> Result<(), String> {
    let guard = state.0.active()?;
    let app = guard.app()?;
    match action.as_str() {
        "submit" => app.submit_manual_run(run_id, &value).map(|_| ()),
        "patch" => app.submit_patch(run_id, &value).map(|_| ()),
        "fail" => app.fail_manual_run(run_id, &value).map(|_| ()),
        _ => Err(anyhow::anyhow!("unknown manual run action: {action}")),
    }
    .map_err(|error| error.to_string())
}

fn workspace_url(app: &OrcApp, agent_id: &str) -> Result<tauri::Url, String> {
    let agent = app
        .agents()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| format!("agent '{agent_id}' not found"))?;
    let configured = orc::registry::manual_workspace_url(&agent)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            format!(
                "manual provider '{}' has no configured workspace URL",
                agent.backend
            )
        })?;
    let url = tauri::Url::parse(&configured)
        .map_err(|error| format!("invalid manual workspace URL: {error}"))?;
    if url.scheme() != "https" || url.host_str().is_none() {
        return Err("manual workspace URL must be an absolute HTTPS URL".into());
    }
    Ok(url)
}

#[tauri::command]
fn manual_workspace_info(
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> ManualWorkspaceInfo {
    let result = state
        .0
        .active()
        .and_then(|guard| workspace_url(guard.app()?, &agent_id));
    match result {
        Ok(url) => ManualWorkspaceInfo {
            supported: true,
            url: Some(url.to_string()),
            error: None,
        },
        Err(error) => ManualWorkspaceInfo {
            supported: false,
            url: None,
            error: Some(error),
        },
    }
}

#[tauri::command]
fn open_manual_workspace(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> Result<(), String> {
    let url = {
        let guard = state.0.active()?;
        workspace_url(guard.app()?, &agent_id)?
    };
    let label = format!(
        "manual-{}",
        agent_id
            .chars()
            .map(|character| if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            })
            .collect::<String>()
    );
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
        .on_navigation(move |candidate| {
            candidate.scheme() == scheme
                && candidate.host_str() == host.as_deref()
                && candidate.port_or_known_default() == port
        })
        .build()
        .map_err(|error| format!("provider could not render in an embedded webview: {error}"))?;
    Ok(())
}

#[tauri::command]
fn close_manual_workspace(app_handle: tauri::AppHandle, agent_id: String) -> Result<(), String> {
    let label = format!(
        "manual-{}",
        agent_id
            .chars()
            .map(|character| if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            })
            .collect::<String>()
    );
    if let Some(window) = app_handle.get_webview_window(&label) {
        window.close().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod project_lifecycle_tests {
    use super::*;
    use tempfile::tempdir;

    fn registered_projects() -> (tempfile::TempDir, project::ProjectRegistry) {
        let dir = tempdir().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir_all(root.join(".orc")).unwrap();
        let database = orc::Database::init(root.join(".orc/orc.db")).unwrap();
        database.create_project("project").unwrap();
        let mut registry =
            project::ProjectRegistry::open(dir.path().join("projects.json")).unwrap();
        registry.register(root, None).unwrap();
        (dir, registry)
    }

    #[test]
    fn removing_active_project_closes_session() {
        let (_dir, mut registry) = registered_projects();
        let project = registry.projects().pop().unwrap();
        let mut session = Some(project::ProjectSession::open(project.clone()).unwrap());

        assert!(remove_project_state(&mut registry, &mut session, &project.id).unwrap());
        assert!(session.is_none());
        assert!(registry.projects().is_empty());
    }

    #[test]
    fn removing_inactive_project_preserves_session() {
        let (_dir, mut registry) = registered_projects();
        let project = registry.projects().pop().unwrap();
        let mut session = None;

        assert!(remove_project_state(&mut registry, &mut session, &project.id).unwrap());
        assert!(session.is_none());
    }

    #[test]
    fn empty_session_returns_error_without_poisoning_lock() {
        let state = SessionState(Mutex::new(None));

        assert!(matches!(state.active(), Err(error) if error == "no active project"));
        assert!(state.active().is_err());
        state.replace(None).unwrap();
    }

    #[test]
    fn session_can_open_close_and_open_again_without_poisoning_lock() {
        let (_dir, registry) = registered_projects();
        let project = registry.projects().pop().unwrap();
        let state = SessionState(Mutex::new(None));

        state
            .replace(Some(
                project::ProjectSession::open(project.clone()).unwrap(),
            ))
            .unwrap();
        assert!(state.active().is_ok());
        state.replace(None).unwrap();
        assert!(matches!(state.active(), Err(error) if error == "no active project"));
        state
            .replace(Some(project::ProjectSession::open(project).unwrap()))
            .unwrap();
        assert!(state.active().is_ok());
        state.replace(None).unwrap();
        assert!(state.active().is_err());
    }
}

#[tauri::command]
fn queue(state: tauri::State<'_, AppState>) -> Result<orc::queue::QueueReport, String> {
    state
        .0
        .active()?
        .app()?
        .queue()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn planning_request(
    state: tauri::State<'_, AppState>,
) -> Result<orc::protocol::PlanningRequest, String> {
    state
        .0
        .active()?
        .app()?
        .planning_request()
        .map_err(|error| error.to_string())
}
#[tauri::command]
fn planner_validate(
    state: tauri::State<'_, AppState>,
    json: String,
) -> Result<orc::protocol::PlanResponse, String> {
    state
        .0
        .active()?
        .app()?
        .validate_plan_json(&json)
        .map_err(|error| error.to_string())
}
#[tauri::command]
fn planner_apply(
    state: tauri::State<'_, AppState>,
    json: String,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    let guard = state.0.active()?;
    let app = guard.app()?;
    let plan = app
        .validate_plan_json(&json)
        .map_err(|error| error.to_string())?;
    app.apply_plan(&plan).map_err(|error| error.to_string())
}
#[tauri::command]
fn approvals(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<orc::storage::db::ApprovalRequest>, String> {
    state
        .0
        .active()?
        .app()?
        .approvals()
        .map_err(|error| error.to_string())
}
#[tauri::command]
fn resolve_approval(state: tauri::State<'_, AppState>, id: i64) -> Result<(), String> {
    state
        .0
        .active()?
        .app()?
        .resolve_approval(id)
        .map_err(|error| error.to_string())
}
#[tauri::command]
fn project_report(
    state: tauri::State<'_, AppState>,
) -> Result<orc::protocol::ProjectReport, String> {
    state
        .0
        .active()?
        .app()?
        .project_report()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn task_details(
    state: tauri::State<'_, AppState>,
    task_id: String,
    activity_limit: usize,
) -> Result<Option<orc::read_model::TaskDetails>, String> {
    state
        .0
        .active()?
        .app()?
        .task_details(&task_id, activity_limit)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn review(
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<orc::review::ReviewSummary, String> {
    state
        .0
        .active()?
        .app()?
        .review(&task_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn dispatch(
    state: tauri::State<'_, AppState>,
    task_id: String,
    agent_id: Option<String>,
) -> Result<orc::review::DispatchSummary, String> {
    state
        .0
        .active()?
        .app()?
        .dispatch(&task_id, agent_id.as_deref())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn task_action(
    state: tauri::State<'_, AppState>,
    action: String,
    task_id: String,
    reason: Option<String>,
    agent_id: Option<String>,
) -> Result<(), String> {
    let guard = state.0.active()?;
    let app = guard.app()?;
    match action.as_str() {
        "dispatch" => app.dispatch(&task_id, agent_id.as_deref()).map(|_| ()),
        "accept" => app.accept(&task_id),
        "reject" => app.reject(&task_id, reason.as_deref()),
        "cancel" => app
            .cancel(&task_id, reason.as_deref())
            .map_err(anyhow::Error::from),
        "requeue" => app.requeue(&task_id),
        "add_dependency" => app.add_dependency(
            &task_id,
            reason
                .as_deref()
                .ok_or_else(|| "dependency id is required".to_string())?,
        ),
        "remove_dependency" => app
            .remove_dependency(
                &task_id,
                reason
                    .as_deref()
                    .ok_or_else(|| "dependency id is required".to_string())?,
            )
            .map(|_| ()),
        "revise" => app.revise(
            &task_id,
            reason
                .as_deref()
                .ok_or_else(|| "feedback is required".to_string())?,
            agent_id
                .as_deref()
                .ok_or_else(|| "agent id is required".to_string())?,
        ),
        _ => Err(anyhow::anyhow!("unknown task action: {action}")),
    }
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn runs(
    state: tauri::State<'_, AppState>,
    limit: usize,
) -> Result<Vec<orc::storage::AgentRun>, String> {
    state
        .0
        .active()?
        .app()?
        .runs(limit)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn runs_workspace(
    state: tauri::State<'_, AppState>,
    limit: usize,
    activity_limit: usize,
) -> Result<orc::read_model::RunsWorkspace, String> {
    state
        .0
        .active()?
        .app()?
        .runs_workspace(limit, activity_limit)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn run_details(
    state: tauri::State<'_, AppState>,
    run_id: i64,
    activity_limit: usize,
) -> Result<Option<orc::read_model::RunDetails>, String> {
    state
        .0
        .active()?
        .app()?
        .run_details(run_id, activity_limit)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn lead_context(
    state: tauri::State<'_, AppState>,
    limit: usize,
) -> Result<orc::lead::LeadContext, String> {
    state
        .0
        .active()?
        .app()?
        .lead()
        .context(limit)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn lead_proposals(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<orc::lead::LeadProposal>, String> {
    state
        .0
        .active()?
        .app()?
        .lead()
        .pending_proposals()
        .map_err(|error| error.to_string())
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
        .active()?
        .app()?
        .invoke_configured_lead(&message, &config, 20)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn apply_lead_proposal(state: tauri::State<'_, AppState>, proposal_id: i64) -> Result<(), String> {
    state
        .0
        .active()?
        .app()?
        .apply_lead_proposal(proposal_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn reject_lead_proposal(state: tauri::State<'_, AppState>, proposal_id: i64) -> Result<(), String> {
    let changed = state
        .0
        .active()?
        .app()?
        .lead()
        .reject_proposal(proposal_id)
        .map_err(|error| error.to_string())?;
    if changed {
        Ok(())
    } else {
        Err("Lead proposal is no longer pending".into())
    }
}

#[cfg(test)]
fn resolve_project_paths(manifest_dir: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let root = manifest_dir
        .parent()
        .filter(|_| {
            manifest_dir
                .file_name()
                .is_some_and(|name| name == "src-tauri")
        })
        .unwrap_or(manifest_dir)
        .to_path_buf();
    let db_path = root.join(".orc/orc.db");
    if !db_path.is_file() {
        anyhow::bail!("project database not found at {}", db_path.display());
    }
    Ok((root, db_path))
}

fn dirs_path(_: &tauri::Config) -> anyhow::Result<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|path| PathBuf::from(path).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|path| PathBuf::from(path).join(".local/share"))
            })
    }
    .context("application data directory is unavailable")?;
    Ok(base.join("orc").join("projects.json"))
}

fn spawn_event_forwarder(
    handle: tauri::AppHandle,
    project_id: String,
    subscription: orc::events::EventSubscription,
    cancellation: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        while let Ok(event) = subscription.recv() {
            if cancellation.load(Ordering::Acquire) {
                break;
            }
            if handle
                .emit(
                    "orc://run-event",
                    ProjectEvent {
                        project_id: &project_id,
                        event,
                    },
                )
                .is_err()
            {
                break;
            }
        }
    });
}

#[derive(Clone, Debug, Serialize)]
struct ProjectEvent<'a> {
    project_id: &'a str,
    event: orc::events::AppEvent,
}

#[cfg(test)]
mod tests {
    use super::resolve_project_paths;
    use std::path::Path;

    #[test]
    fn project_root_is_the_parent_of_src_tauri() {
        let result = resolve_project_paths(Path::new("/workspace/orc/src-tauri"));
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "project database not found at /workspace/orc/.orc/orc.db"
        );
    }

    #[test]
    fn missing_project_database_is_reported() {
        let result = resolve_project_paths(Path::new("/workspace/orc"));
        assert!(result.is_err());
    }
}

pub fn run() -> anyhow::Result<()> {
    let app_data = tauri::Config::default();
    let registry_path = dirs_path(&app_data)?;
    let registry = project::ProjectRegistry::open(&registry_path)?;
    let builder = tauri::Builder::default();
    builder
        .manage(AppState(SessionState(Mutex::new(None))))
        .manage(RegistryState(Mutex::new(registry)))
        .invoke_handler(tauri::generate_handler![
            registered_projects,
            register_project,
            import_project,
            adopt_project,
            project_availability,
            relocate_project,
            remove_project,
            open_project,
            close_project,
            current_project,
            snapshot,
            tasks,
            agents,
            configure_agent,
            sync_agent,
            manual_runs,
            manual_run_action,
            manual_workspace_info,
            open_manual_workspace,
            close_manual_workspace,
            queue,
            planning_request,
            planner_validate,
            planner_apply,
            approvals,
            resolve_approval,
            project_report,
            task_details,
            review,
            dispatch,
            task_action,
            runs,
            runs_workspace,
            run_details,
            lead_context,
            lead_proposals,
            invoke_lead,
            apply_lead_proposal,
            reject_lead_proposal
        ])
        .run(tauri::generate_context!())?;
    Ok(())
}
