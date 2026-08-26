use crate::app::OrcApp;
use crate::events::AppEvent;
use crate::queue::QueueReport;
use crate::review::DispatchSummary;
use crate::storage::AgentRun;
use crate::storage::Database;
use crate::task::Task;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OperationId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeRequest {
    ProjectStatus,
    Tasks,
    TaskShow(String),
    Queue,
    Runs(usize),
    Agents,
    DispatchCandidates(String),
    Dispatch {
        task_id: String,
        agent_id: Option<String>,
    },
    CancelTask(String),
}

#[derive(Debug, Clone)]
pub enum RuntimeValue {
    Status(String),
    Tasks(Vec<Task>),
    Task(Option<Task>),
    Queue(QueueReport),
    Runs(Vec<AgentRun>),
    Agents(Vec<crate::registry::AgentDefinition>),
    AgentCandidates {
        task_id: String,
        agents: Vec<String>,
    },
    Dispatch(Box<DispatchSummary>),
    Cancelled(bool),
}

#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    Context(OperationId, SessionContext),
    Started(OperationId),
    Lifecycle(OperationId, AppEvent),
    Completed(OperationId, Box<RuntimeValue>),
    Failed(OperationId, String),
    Cancelled(OperationId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionContext {
    pub project: Option<String>,
}

#[derive(Clone)]
pub struct Cancellation {
    control: crate::worker::CancellationControl,
}

impl Cancellation {
    pub fn new() -> Self {
        Self {
            control: crate::worker::CancellationControl::new(),
        }
    }
    pub fn request(&self) {
        self.control.cancel();
    }
    pub fn is_requested(&self) -> bool {
        self.control.is_cancelled()
    }
}

impl Default for Cancellation {
    fn default() -> Self {
        Self::new()
    }
}

struct Envelope {
    id: OperationId,
    request: RuntimeRequest,
    cancellation: Cancellation,
}

pub struct Runtime {
    requests: mpsc::Sender<Envelope>,
    events: mpsc::Receiver<RuntimeEvent>,
    next_id: u64,
}

impl Runtime {
    pub fn open(db_path: impl AsRef<Path>, repo_path: impl AsRef<Path>) -> Result<Self> {
        let (requests, incoming) = mpsc::channel();
        let (outgoing, events) = mpsc::channel();
        let db_path = db_path.as_ref().to_path_buf();
        let repo_path = repo_path.as_ref().to_path_buf();
        thread::Builder::new()
            .name("orc-app-owner".into())
            .spawn(move || owner(incoming, outgoing, db_path, repo_path))?;
        Ok(Self {
            requests,
            events,
            next_id: 0,
        })
    }

    pub fn submit(&mut self, request: RuntimeRequest) -> Result<(OperationId, Cancellation)> {
        self.next_id += 1;
        let id = OperationId(self.next_id);
        let cancellation = Cancellation {
            control: crate::worker::CancellationControl::new(),
        };
        self.requests
            .send(Envelope {
                id,
                request,
                cancellation: cancellation.clone(),
            })
            .context("submit runtime request")?;
        Ok((id, cancellation))
    }

    pub fn recv(&self) -> Result<RuntimeEvent, mpsc::RecvError> {
        self.events.recv()
    }
    pub fn try_recv(&self) -> Result<RuntimeEvent, mpsc::TryRecvError> {
        self.events.try_recv()
    }

    pub fn cancel(&self, cancellation: &Cancellation) {
        cancellation.request();
    }
}

fn owner(
    incoming: mpsc::Receiver<Envelope>,
    outgoing: mpsc::Sender<RuntimeEvent>,
    db: PathBuf,
    repo: PathBuf,
) {
    let app = match OrcApp::open(&db, &repo) {
        Ok(app) => app,
        Err(_) => match (|| -> anyhow::Result<OrcApp> {
            Database::init(&db)?;
            OrcApp::open(&db, &repo)
        })() {
            Ok(app) => app,
            Err(error) => {
                let _ = outgoing.send(RuntimeEvent::Failed(OperationId(0), error.to_string()));
                return;
            }
        },
    };
    while let Ok(envelope) = incoming.recv() {
        let id = envelope.id;
        let _ = outgoing.send(RuntimeEvent::Started(id));
        let context = match app_context(&app) {
            Ok(context) => context,
            Err(error) => {
                let _ = outgoing.send(RuntimeEvent::Failed(id, error.to_string()));
                continue;
            }
        };
        let _ = outgoing.send(RuntimeEvent::Context(id, context));
        if envelope.cancellation.is_requested() {
            if supports_cancellation(&envelope.request) {
                let _ = outgoing.send(RuntimeEvent::Cancelled(id));
            } else {
                let _ = outgoing.send(RuntimeEvent::Failed(
                    id,
                    "cancellation is unsupported for this operation".into(),
                ));
            }
            continue;
        }
        let subscription = app.subscribe();
        let (done_tx, done_rx) = mpsc::channel();
        let event_out = outgoing.clone();
        let event_thread = thread::spawn(move || {
            loop {
                match subscription.recv_timeout(std::time::Duration::from_millis(10)) {
                    Ok(event) => {
                        if event_out.send(RuntimeEvent::Lifecycle(id, event)).is_err() {
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if done_rx.try_recv().is_ok() {
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        let result = execute(&app, &envelope.request, &envelope.cancellation);
        let _ = done_tx.send(());
        let _ = event_thread.join();
        if envelope.cancellation.is_requested() && supports_cancellation(&envelope.request) {
            let _ = outgoing.send(RuntimeEvent::Cancelled(id));
        } else if envelope.cancellation.is_requested() {
            let _ = outgoing.send(RuntimeEvent::Failed(
                id,
                "cancellation is unsupported for this operation".into(),
            ));
        } else {
            match result {
                Ok(value) => {
                    let _ = outgoing.send(RuntimeEvent::Completed(id, Box::new(value)));
                }
                Err(error) => {
                    let _ = outgoing.send(RuntimeEvent::Failed(id, error.to_string()));
                }
            }
        }
    }
}

fn supports_cancellation(request: &RuntimeRequest) -> bool {
    matches!(request, RuntimeRequest::Dispatch { .. })
}

fn app_context(app: &OrcApp) -> Result<SessionContext> {
    Ok(SessionContext {
        project: app.project_report().ok().map(|report| report.project.name),
    })
}

fn execute(
    app: &OrcApp,
    request: &RuntimeRequest,
    cancellation: &Cancellation,
) -> Result<RuntimeValue> {
    if cancellation.is_requested() {
        anyhow::bail!("operation cancelled")
    }
    Ok(match request {
        RuntimeRequest::ProjectStatus => match app.project_health() {
            Ok(health) => RuntimeValue::Status(format!(
                "{} active runs, {} unresolved approvals",
                health.active_runs, health.unresolved_approvals
            )),
            Err(_) => RuntimeValue::Status("no active project".into()),
        },
        RuntimeRequest::Tasks => RuntimeValue::Tasks(app.tasks()?),
        RuntimeRequest::TaskShow(id) => RuntimeValue::Task(app.task(id)?),
        RuntimeRequest::Queue => RuntimeValue::Queue(app.queue()?),
        RuntimeRequest::Runs(limit) => RuntimeValue::Runs(app.runs(*limit)?),
        RuntimeRequest::Agents => RuntimeValue::Agents(app.agents()?),
        RuntimeRequest::DispatchCandidates(task_id) => {
            let task = app.task(task_id)?.context("task not found")?;
            let agents = app.agents()?;
            let decision = crate::scheduler::schedule(&task, &agents, None)?;
            let eligible = decision
                .candidates
                .into_iter()
                .filter(|candidate| {
                    matches!(
                        candidate.status,
                        crate::scheduler::CandidateStatus::Eligible
                    )
                })
                .map(|candidate| candidate.agent_id)
                .collect();
            RuntimeValue::AgentCandidates {
                task_id: task_id.clone(),
                agents: eligible,
            }
        }
        RuntimeRequest::Dispatch { task_id, agent_id } => RuntimeValue::Dispatch(Box::new(
            app.dispatch_cancellable(task_id, agent_id.as_deref(), &cancellation.control)?,
        )),
        RuntimeRequest::CancelTask(id) => {
            app.cancel(id, None)
                .map_err(|error| anyhow::anyhow!(error))?;
            RuntimeValue::Cancelled(true)
        }
    })
}

pub fn render_event(event: &RuntimeEvent) -> String {
    match event {
        RuntimeEvent::Context(_, context) => match &context.project {
            Some(project) => format!("project: {project}\r\n"),
            None => "project: none\r\n".into(),
        },
        RuntimeEvent::Started(id) => format!("operation {} started\r\n", id.0),
        RuntimeEvent::Lifecycle(_, event) => format!("progress: {}\r\n", lifecycle_text(event)),
        RuntimeEvent::Completed(_, value) => format!("success: {}\r\n", value_text(value)),
        RuntimeEvent::Failed(_, error) => format!("error: {error}\r\n"),
        RuntimeEvent::Cancelled(_) => "operation cancelled\r\n".into(),
    }
}

fn lifecycle_text(event: &AppEvent) -> String {
    match event {
        AppEvent::WorkerOutput(event) => {
            format!("worker: {}", event.payload.as_deref().unwrap_or("output"))
        }
        AppEvent::RunPhaseChanged(event) => {
            format!("phase: {}", event.payload.as_deref().unwrap_or("changed"))
        }
        _ => "application state changed".into(),
    }
}

fn value_text(value: &RuntimeValue) -> String {
    match value {
        RuntimeValue::Status(status) => status.clone(),
        RuntimeValue::Tasks(tasks) => format!("{} task(s)", tasks.len()),
        RuntimeValue::Task(task) => {
            if task.is_some() {
                "task found".into()
            } else {
                "task not found".into()
            }
        }
        RuntimeValue::Queue(queue) => format!("{} queued task(s)", queue.all_items().len()),
        RuntimeValue::Runs(runs) => format!("{} run(s)", runs.len()),
        RuntimeValue::Agents(agents) => format!("{} agent(s)", agents.len()),
        RuntimeValue::AgentCandidates { agents, .. } => {
            format!("{} eligible agent(s)", agents.len())
        }
        RuntimeValue::Dispatch(_) => "dispatch submitted".into(),
        RuntimeValue::Cancelled(_) => "task cancellation applied".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn runtime() -> (tempfile::TempDir, Runtime) {
        let directory = tempdir().unwrap();
        std::fs::create_dir(directory.path().join(".orc")).unwrap();
        std::fs::write(
            directory.path().join(".orc/engineering.md"),
            "# Engineering Contract\n\n## General\n- Keep changes scoped.\n",
        )
        .unwrap();
        let database = directory.path().join("orc.db");
        let runtime = Runtime::open(&database, directory.path()).unwrap();
        (directory, runtime)
    }

    fn events_until(runtime: &Runtime, id: OperationId) -> Vec<RuntimeEvent> {
        let mut events = Vec::new();
        loop {
            let event = runtime.recv().unwrap();
            let complete = matches!(
                &event,
                RuntimeEvent::Completed(event_id, _)
                    | RuntimeEvent::Failed(event_id, _)
                    | RuntimeEvent::Cancelled(event_id)
                    if *event_id == id
            );
            events.push(event);
            if complete {
                return events;
            }
        }
    }

    #[test]
    fn operation_ids_are_unique_and_cancellation_is_shared() {
        let (sender, _receiver) = mpsc::channel();
        let (_outgoing, events) = mpsc::channel();
        let mut runtime = Runtime {
            requests: sender,
            events,
            next_id: 0,
        };
        let (first, cancellation) = runtime.submit(RuntimeRequest::Tasks).unwrap();
        let (second, _) = runtime.submit(RuntimeRequest::Queue).unwrap();
        assert_eq!(first, OperationId(1));
        assert_eq!(second, OperationId(2));
        assert!(!cancellation.is_requested());
        cancellation.request();
        assert!(cancellation.is_requested());
    }

    #[test]
    fn structured_errors_render_without_panicking() {
        assert_eq!(
            render_event(&RuntimeEvent::Failed(OperationId(4), "bad state".into())),
            "error: bad state\r\n"
        );
        assert_eq!(
            render_event(&RuntimeEvent::Cancelled(OperationId(4))),
            "operation cancelled\r\n"
        );
    }

    #[test]
    fn requests_and_results_keep_their_operation_ids() {
        let (_directory, mut runtime) = runtime();
        let (first, _) = runtime.submit(RuntimeRequest::Tasks).unwrap();
        let first_events = events_until(&runtime, first);
        let (second, _) = runtime.submit(RuntimeRequest::ProjectStatus).unwrap();
        let second_events = events_until(&runtime, second);
        assert!(
            first_events
                .iter()
                .all(|event| event_id(event) != Some(second))
        );
        assert!(
            second_events
                .iter()
                .all(|event| event_id(event) != Some(first))
        );
        assert!(
            matches!(first_events.last(), Some(RuntimeEvent::Completed(id, _)) if *id == first)
        );
        assert!(
            matches!(second_events.last(), Some(RuntimeEvent::Completed(id, _)) if *id == second)
        );
    }

    #[test]
    fn startup_context_supports_empty_and_active_projects() {
        let (directory, mut runtime) = runtime();
        let (empty_id, _) = runtime.submit(RuntimeRequest::ProjectStatus).unwrap();
        let empty = events_until(&runtime, empty_id);
        assert!(empty.iter().any(|event| matches!(event, RuntimeEvent::Context(id, context) if *id == empty_id && context.project.is_none())));

        let database = Database::init(directory.path().join("orc.db")).unwrap();
        database.create_project("acceptance-project").unwrap();
        let (active_id, _) = runtime.submit(RuntimeRequest::ProjectStatus).unwrap();
        let active = events_until(&runtime, active_id);
        assert!(active.iter().any(|event| matches!(event, RuntimeEvent::Context(id, context) if *id == active_id && context.project.as_deref() == Some("acceptance-project"))));
    }

    #[test]
    fn application_errors_are_structured_and_session_can_continue() {
        let (_directory, mut runtime) = runtime();
        let (bad_id, _) = runtime
            .submit(RuntimeRequest::DispatchCandidates("missing".into()))
            .unwrap();
        let bad = events_until(&runtime, bad_id);
        assert!(
            matches!(bad.last(), Some(RuntimeEvent::Failed(id, message)) if *id == bad_id && !message.is_empty())
        );
        let (good_id, _) = runtime.submit(RuntimeRequest::ProjectStatus).unwrap();
        assert!(
            matches!(events_until(&runtime, good_id).last(), Some(RuntimeEvent::Completed(id, _)) if *id == good_id)
        );
    }

    #[test]
    fn unsupported_cancellation_is_an_error_not_a_false_cancelled_event() {
        let (_directory, mut runtime) = runtime();
        let (id, cancellation) = runtime.submit(RuntimeRequest::ProjectStatus).unwrap();
        cancellation.request();
        let events = events_until(&runtime, id);
        assert!(
            matches!(events.last(), Some(RuntimeEvent::Failed(event_id, message)) if *event_id == id && message.contains("unsupported"))
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, RuntimeEvent::Cancelled(event_id) if *event_id == id))
        );
    }

    #[test]
    fn submit_reports_disconnected_owner_without_panicking() {
        let (requests, incoming) = mpsc::channel();
        drop(incoming);
        let (_events_sender, events) = mpsc::channel();
        let mut runtime = Runtime {
            requests,
            events,
            next_id: 0,
        };

        let error = match runtime.submit(RuntimeRequest::Tasks) {
            Ok(_) => panic!("disconnected owner unexpectedly accepted request"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("submit runtime request"));
    }

    #[test]
    fn lifecycle_is_rendered_before_completion_when_published() {
        let event = RuntimeEvent::Lifecycle(
            OperationId(9),
            AppEvent::RunPhaseChanged(crate::storage::db::LifecycleEvent {
                id: 1,
                timestamp: "now".into(),
                kind: "run_phase_changed".into(),
                task_id: None,
                run_id: None,
                agent_id: None,
                payload: Some("working".into()),
            }),
        );
        assert!(render_event(&event).contains("phase: working"));
        assert!(
            render_event(&RuntimeEvent::Completed(
                OperationId(9),
                Box::new(RuntimeValue::Status("done".into()))
            ))
            .contains("success: done")
        );
    }

    fn event_id(event: &RuntimeEvent) -> Option<OperationId> {
        match event {
            RuntimeEvent::Context(id, _)
            | RuntimeEvent::Started(id)
            | RuntimeEvent::Lifecycle(id, _)
            | RuntimeEvent::Completed(id, _)
            | RuntimeEvent::Failed(id, _)
            | RuntimeEvent::Cancelled(id) => Some(*id),
        }
    }
}
