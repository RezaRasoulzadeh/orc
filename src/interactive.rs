//! Interactive session: rustyline owns terminal mechanics and dialoguer owns
//! confirmations/selections; Orc owns parsing and Runtime orchestration.
use crate::runtime::{Runtime, RuntimeEvent, RuntimeRequest, RuntimeValue, render_event};
use anyhow::{Context, Result};
use dialoguer::{Confirm, Select};
use rustyline::error::ReadlineError;
use rustyline::{DefaultEditor, ExternalPrinter};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

pub fn parse_arguments(line: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    for ch in line.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (None, '\'' | '"') => {
                quote = Some(ch);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (_, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        anyhow::bail!("unterminated quote")
    }
    if started {
        out.push(word);
    }
    Ok(out)
}

fn request(args: &[String]) -> Option<RuntimeRequest> {
    match args {
        [c] if c == "status" || c == "project/status" => Some(RuntimeRequest::ProjectStatus),
        [c] if c == "tasks" => Some(RuntimeRequest::Tasks),
        [c] if c == "queue" => Some(RuntimeRequest::Queue),
        [c] if c == "runs" => Some(RuntimeRequest::Runs(20)),
        [c] if c == "agents" => Some(RuntimeRequest::Agents),
        [c, id] if c == "task" || c == "task/show" => Some(RuntimeRequest::TaskShow(id.clone())),
        [c, id] if c == "cancel" => Some(RuntimeRequest::CancelTask(id.clone())),
        [c, id] if c == "dispatch" => Some(RuntimeRequest::Dispatch {
            task_id: id.clone(),
            agent_id: None,
        }),
        [c, id, agent] if c == "dispatch" => Some(RuntimeRequest::Dispatch {
            task_id: id.clone(),
            agent_id: Some(agent.clone()),
        }),
        _ => None,
    }
}

#[derive(Clone)]
struct ActiveOperation {
    id: crate::runtime::OperationId,
    cancellation: crate::runtime::Cancellation,
}

struct ShutdownGuard(Arc<AtomicBool>);

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn terminal_event(event: &RuntimeEvent, id: crate::runtime::OperationId) -> bool {
    matches!(event, RuntimeEvent::Completed(event_id, _) | RuntimeEvent::Failed(event_id, _) | RuntimeEvent::Cancelled(event_id) if *event_id == id)
}

trait SessionRuntime {
    fn submit(
        &self,
        request: RuntimeRequest,
    ) -> Result<(crate::runtime::OperationId, crate::runtime::Cancellation)>;
    fn try_recv(&self) -> Result<RuntimeEvent, mpsc::TryRecvError>;
}

impl SessionRuntime for Runtime {
    fn submit(
        &self,
        request: RuntimeRequest,
    ) -> Result<(crate::runtime::OperationId, crate::runtime::Cancellation)> {
        Runtime::submit(self, request)
    }
    fn try_recv(&self) -> Result<RuntimeEvent, mpsc::TryRecvError> {
        Runtime::try_recv(self)
    }
}

trait SessionInput {
    fn readline(&mut self, prompt: &str) -> std::result::Result<String, ReadlineError>;
    fn history(&self) -> Vec<String> {
        Vec::new()
    }
    fn add_history(&mut self, _line: &str) -> Result<()> {
        Ok(())
    }
}

trait SessionPrinter {
    fn emit(&mut self, text: String) -> Result<()>;
}
trait SessionDialogs {
    fn confirm(&mut self, prompt: &str) -> Result<bool>;
    fn select(&mut self, prompt: &str, items: &[String]) -> Result<usize>;
}

impl SessionInput for DefaultEditor {
    fn readline(&mut self, prompt: &str) -> std::result::Result<String, ReadlineError> {
        self.readline(prompt)
    }
    fn history(&self) -> Vec<String> {
        self.history().iter().cloned().collect()
    }
    fn add_history(&mut self, line: &str) -> Result<()> {
        self.add_history_entry(line).map(|_| ()).map_err(Into::into)
    }
}
impl<T: ExternalPrinter> SessionPrinter for T {
    fn emit(&mut self, text: String) -> Result<()> {
        ExternalPrinter::print(self, text).map_err(|e| anyhow::anyhow!(e))
    }
}
struct DialoguerDialogs;
impl SessionDialogs for DialoguerDialogs {
    fn confirm(&mut self, prompt: &str) -> Result<bool> {
        Ok(Confirm::new()
            .with_prompt(prompt)
            .default(false)
            .interact()?)
    }
    fn select(&mut self, prompt: &str, items: &[String]) -> Result<usize> {
        Ok(Select::new()
            .with_prompt(prompt)
            .items(items)
            .default(0)
            .interact()?)
    }
}

fn set_active(active: &Arc<Mutex<Option<ActiveOperation>>>, operation: Option<ActiveOperation>) {
    *active.lock().expect("active lock") = operation;
}

fn run_session<
    R: SessionRuntime + Sync,
    I: SessionInput,
    P: SessionPrinter + Send,
    D: SessionDialogs,
>(
    runtime: &R,
    input: &mut I,
    printer: Arc<Mutex<P>>,
    dialogs: &mut D,
) -> Result<()> {
    let (done_tx, done_rx) = mpsc::channel();
    let active = Arc::new(Mutex::new(None::<ActiveOperation>));
    let shutting_down = Arc::new(AtomicBool::new(false));
    let active_events = active.clone();
    let stopping = shutting_down.clone();
    let event_printer = printer.clone();
    thread::scope(|scope| -> Result<()> {
        let event_thread = scope.spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                let event = match runtime.try_recv() {
                    Ok(event) => event,
                    Err(mpsc::TryRecvError::Empty) => {
                        thread::sleep(std::time::Duration::from_millis(2));
                        continue;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => break,
                };
                let done = matches!(
                    event,
                    RuntimeEvent::Completed(..)
                        | RuntimeEvent::Failed(..)
                        | RuntimeEvent::Cancelled(..)
                );
                let _ = event_printer
                    .lock()
                    .expect("printer lock")
                    .emit(render_event(&event));
                if done {
                    let _ = done_tx.send(event.clone());
                    let current_id = {
                        active_events
                            .lock()
                            .expect("active lock")
                            .as_ref()
                            .map(|op| op.id)
                    };
                    if current_id.is_some_and(|id| terminal_event(&event, id)) {
                        set_active(&active_events, None);
                    }
                }
            }
        });
        let result = (|| -> Result<()> {
            let (context_id, context_cancel) = runtime.submit(RuntimeRequest::ProjectStatus)?;
            set_active(
                &active,
                Some(ActiveOperation {
                    id: context_id,
                    cancellation: context_cancel,
                }),
            );
            while let Ok(event) = done_rx.recv() {
                if terminal_event(&event, context_id) {
                    break;
                }
            }
            loop {
                match input.readline("orc> ") {
                    Ok(line) => {
                        if !line.trim().is_empty() {
                            input.add_history(&line)?;
                        }
                        let args = match parse_arguments(&line) {
                            Ok(a) => a,
                            Err(e) => {
                                printer
                                    .lock()
                                    .expect("printer lock")
                                    .emit(format!("error: {e}\n"))?;
                                continue;
                            }
                        };
                        if matches!(args.first().map(String::as_str), Some("exit" | "quit")) {
                            break;
                        }
                        if args.first().is_some_and(|a| a == "help") {
                            printer
                                .lock()
                                .expect("printer lock")
                                .emit("help history clear exit quit\n".into())?;
                            continue;
                        }
                        if args.first().is_some_and(|a| a == "history") {
                            for (i, entry) in input.history().iter().enumerate() {
                                printer.lock().expect("printer lock").emit(format!(
                                    "{}  {}\n",
                                    i + 1,
                                    entry
                                ))?;
                            }
                            continue;
                        }
                        if args.first().is_some_and(|a| a == "clear") {
                            printer
                                .lock()
                                .expect("printer lock")
                                .emit("\x1b[2J\x1b[H".into())?;
                            continue;
                        }
                        if args.first().is_some_and(|a| a == "cancel") {
                            let task = args.get(1).context("cancel requires a task id")?;
                            if dialogs.confirm("confirm task cancellation?")? {
                                let (id, cancellation) =
                                    runtime.submit(RuntimeRequest::CancelTask(task.clone()))?;
                                set_active(&active, Some(ActiveOperation { id, cancellation }));
                            }
                            continue;
                        }
                        if args.len() == 2 && args[0] == "dispatch" {
                            let (id, cancellation) = runtime
                                .submit(RuntimeRequest::DispatchCandidates(args[1].clone()))?;
                            set_active(&active, Some(ActiveOperation { id, cancellation }));
                            loop {
                                match done_rx.recv_timeout(std::time::Duration::from_millis(10)) {
                                    Ok(event) => match event {
                                        RuntimeEvent::Completed(event_id, value)
                                            if event_id == id =>
                                        {
                                            if let RuntimeValue::AgentCandidates {
                                                task_id,
                                                agents,
                                            } = *value
                                            {
                                                if agents.is_empty() {
                                                    printer.lock().expect("printer lock").emit(
                                                        "error: no eligible agents available\n"
                                                            .into(),
                                                    )?;
                                                    set_active(&active, None);
                                                    break;
                                                }
                                                let i = if agents.len() == 1 {
                                                    0
                                                } else {
                                                    dialogs
                                                        .select("select dispatch agent", &agents)?
                                                };
                                                let (did, dc) =
                                                    runtime.submit(RuntimeRequest::Dispatch {
                                                        task_id,
                                                        agent_id: Some(agents[i].clone()),
                                                    })?;
                                                set_active(
                                                    &active,
                                                    Some(ActiveOperation {
                                                        id: did,
                                                        cancellation: dc,
                                                    }),
                                                );
                                            }
                                            break;
                                        }
                                        RuntimeEvent::Failed(eid, error) if eid == id => {
                                            printer
                                                .lock()
                                                .expect("printer lock")
                                                .emit(format!("error: {error}\n"))?;
                                            break;
                                        }
                                        RuntimeEvent::Cancelled(eid) if eid == id => {
                                            printer
                                                .lock()
                                                .expect("printer lock")
                                                .emit("operation cancelled\n".into())?;
                                            break;
                                        }
                                        _ => {}
                                    },
                                    Err(mpsc::RecvTimeoutError::Timeout) => {
                                        match input.readline("orc> ") {
                                            Ok(_) => {}
                                            Err(ReadlineError::Interrupted) => {
                                                if let Some(op) =
                                                    active.lock().expect("active lock").as_ref()
                                                {
                                                    op.cancellation.request();
                                                }
                                            }
                                            Err(ReadlineError::Eof) => break,
                                            Err(error) => return Err(error.into()),
                                        }
                                    }
                                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                                }
                            }
                        } else if let Some(request) = request(&args) {
                            let (id, cancellation) = runtime.submit(request)?;
                            set_active(&active, Some(ActiveOperation { id, cancellation }));
                        }
                    }
                    Err(ReadlineError::Interrupted) => {
                        if let Some(op) = active.lock().expect("active lock").as_ref() {
                            op.cancellation.request();
                        }
                    }
                    Err(ReadlineError::Eof) => break,
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(())
        })();
        // This must happen before leaving the scope: scoped threads are joined
        // as the scope exits, including when the session returned an error.
        let _shutdown = ShutdownGuard(shutting_down.clone());
        drop(_shutdown);
        event_thread
            .join()
            .map_err(|_| anyhow::anyhow!("event thread panicked"))?;
        result
    })
}

fn editor_with_history(history_path: &Path) -> Result<DefaultEditor> {
    let mut editor = DefaultEditor::new().context("create interactive editor")?;
    if history_path.exists() {
        editor
            .load_history(history_path)
            .context("load interactive history")?;
    }
    Ok(editor)
}

fn save_editor_history(editor: &mut DefaultEditor, history_path: &Path) -> Result<()> {
    editor
        .save_history(history_path)
        .context("save interactive history")?;
    Ok(())
}

pub fn run() -> Result<()> {
    let runtime = Runtime::open(".orc/orc.db", ".")?;
    let history_path = Path::new(".orc/history");
    let mut editor = editor_with_history(history_path)?;
    let printer = Arc::new(Mutex::new(
        editor
            .create_external_printer()
            .context("create event printer")?,
    ));
    let result = run_session(&runtime, &mut editor, printer, &mut DialoguerDialogs);
    if result.is_ok() {
        save_editor_history(&mut editor, history_path)?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use tempfile::tempdir;

    struct ScriptInput(VecDeque<std::result::Result<String, ReadlineError>>);
    impl SessionInput for ScriptInput {
        fn readline(&mut self, _: &str) -> std::result::Result<String, ReadlineError> {
            self.0.pop_front().unwrap_or(Err(ReadlineError::Eof))
        }
    }
    type InputAction = Box<dyn FnMut() -> std::result::Result<String, ReadlineError>>;
    struct CoordinatedInput(VecDeque<InputAction>);
    impl SessionInput for CoordinatedInput {
        fn readline(&mut self, _: &str) -> std::result::Result<String, ReadlineError> {
            self.0
                .pop_front()
                .map(|mut action| action())
                .unwrap_or(Err(ReadlineError::Eof))
        }
    }
    #[derive(Default)]
    struct Capture(Arc<Mutex<Vec<String>>>);
    impl SessionPrinter for Capture {
        fn emit(&mut self, text: String) -> Result<()> {
            self.0.lock().unwrap().push(text);
            Ok(())
        }
    }
    #[derive(Default)]
    struct SignallingCapture(Arc<(Mutex<Vec<String>>, std::sync::Condvar)>);
    impl SessionPrinter for SignallingCapture {
        fn emit(&mut self, text: String) -> Result<()> {
            let (output, emitted) = &*self.0;
            output.lock().unwrap().push(text);
            emitted.notify_all();
            Ok(())
        }
    }
    struct ScriptRuntime {
        requests: Arc<Mutex<Vec<RuntimeRequest>>>,
        events: Arc<Mutex<VecDeque<RuntimeEvent>>>,
        cancellations: Arc<(Mutex<Vec<crate::runtime::Cancellation>>, std::sync::Condvar)>,
        next: Mutex<u64>,
    }
    impl ScriptRuntime {
        fn new() -> Self {
            Self {
                requests: Default::default(),
                events: Default::default(),
                cancellations: Arc::new((Mutex::new(Vec::new()), std::sync::Condvar::new())),
                next: Mutex::new(0),
            }
        }
        fn queue(&self, event: RuntimeEvent) {
            self.events.lock().unwrap().push_back(event);
        }
    }
    impl SessionRuntime for ScriptRuntime {
        fn submit(
            &self,
            request: RuntimeRequest,
        ) -> Result<(crate::runtime::OperationId, crate::runtime::Cancellation)> {
            self.requests.lock().unwrap().push(request);
            let mut next = self.next.lock().unwrap();
            *next += 1;
            let cancellation = crate::runtime::Cancellation::new();
            let (cancellations, submitted) = &*self.cancellations;
            cancellations.lock().unwrap().push(cancellation.clone());
            submitted.notify_all();
            Ok((crate::runtime::OperationId(*next), cancellation))
        }
        fn try_recv(&self) -> Result<RuntimeEvent, mpsc::TryRecvError> {
            self.events
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(mpsc::TryRecvError::Empty)
        }
    }
    struct ScriptDialogs {
        confirms: VecDeque<bool>,
        selections: VecDeque<usize>,
        seen: Vec<Vec<String>>,
    }
    impl SessionDialogs for ScriptDialogs {
        fn confirm(&mut self, _: &str) -> Result<bool> {
            Ok(self.confirms.pop_front().unwrap_or(false))
        }
        fn select(&mut self, _: &str, items: &[String]) -> Result<usize> {
            self.seen.push(items.to_vec());
            Ok(self.selections.pop_front().unwrap_or(0))
        }
    }
    fn completed(id: u64, value: RuntimeValue) -> RuntimeEvent {
        RuntimeEvent::Completed(crate::runtime::OperationId(id), Box::new(value))
    }
    fn session(
        input: Vec<std::result::Result<String, ReadlineError>>,
        runtime: &ScriptRuntime,
        dialogs: &mut ScriptDialogs,
    ) -> Arc<Mutex<Vec<String>>> {
        let output = Arc::new(Mutex::new(Vec::new()));
        run_session(
            runtime,
            &mut ScriptInput(input.into()),
            Arc::new(Mutex::new(Capture(output.clone()))),
            dialogs,
        )
        .unwrap();
        output
    }
    fn dialogs() -> ScriptDialogs {
        ScriptDialogs {
            confirms: VecDeque::new(),
            selections: VecDeque::new(),
            seen: Vec::new(),
        }
    }
    fn lifecycle(payload: &str) -> crate::events::AppEvent {
        crate::events::AppEvent::WorkerOutput(crate::storage::db::LifecycleEvent {
            id: 1,
            timestamp: "2026-01-01T00:00:00Z".into(),
            kind: "worker_output".into(),
            task_id: Some("T-0001".into()),
            run_id: Some(1),
            agent_id: Some("codex-main".into()),
            payload: Some(payload.into()),
        })
    }
    #[test]
    fn parses_quotes() {
        assert_eq!(
            parse_arguments("run 'two words' \"\"").unwrap(),
            ["run", "two words", ""]
        );
    }
    #[test]
    fn maps_dispatch() {
        assert_eq!(
            request(&["dispatch".into(), "T-1".into()]),
            Some(RuntimeRequest::Dispatch {
                task_id: "T-1".into(),
                agent_id: None
            })
        );
    }

    #[test]
    fn history_startup_without_file_uses_empty_history() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("history");

        let editor = editor_with_history(&path).unwrap();

        assert!(!path.exists());
        assert!(editor.history().iter().next().is_none());
    }

    #[test]
    fn history_startup_loads_existing_entries_in_order() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("history");
        let mut writer = editor_with_history(&path).unwrap();
        writer.add_history_entry("first").unwrap();
        writer.add_history_entry("second").unwrap();
        save_editor_history(&mut writer, &path).unwrap();

        let editor = editor_with_history(&path).unwrap();

        assert_eq!(
            editor.history().iter().collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[test]
    fn history_save_persists_commands_added_to_session_editor() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("history");
        let mut editor = editor_with_history(&path).unwrap();
        SessionInput::add_history(&mut editor, "status").unwrap();
        SessionInput::add_history(&mut editor, "tasks --unicode café").unwrap();

        save_editor_history(&mut editor, &path).unwrap();
        let reopened = editor_with_history(&path).unwrap();

        assert_eq!(
            reopened.history().iter().collect::<Vec<_>>(),
            ["status", "tasks --unicode café",]
        );
    }

    #[test]
    fn history_round_trip_restores_entries_after_session_close() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("history");
        {
            let mut session = editor_with_history(&path).unwrap();
            SessionInput::add_history(&mut session, "dispatch T-0001").unwrap();
            save_editor_history(&mut session, &path).unwrap();
        }

        let reopened = editor_with_history(&path).unwrap();

        assert_eq!(
            reopened.history().iter().collect::<Vec<_>>(),
            ["dispatch T-0001"]
        );
    }

    #[test]
    fn history_save_preserves_existing_entries_and_appends_new_commands() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("history");
        let mut editor = editor_with_history(&path).unwrap();
        SessionInput::add_history(&mut editor, "old command").unwrap();
        save_editor_history(&mut editor, &path).unwrap();

        let mut session = editor_with_history(&path).unwrap();
        SessionInput::add_history(&mut session, "new command").unwrap();
        save_editor_history(&mut session, &path).unwrap();
        let reopened = editor_with_history(&path).unwrap();

        assert_eq!(
            reopened.history().iter().collect::<Vec<_>>(),
            ["old command", "new command"]
        );
    }

    #[test]
    fn active_interrupt_requests_runtime_cancellation() {
        let cancellation = crate::runtime::Cancellation::new();
        let active = ActiveOperation {
            id: crate::runtime::OperationId(1),
            cancellation: cancellation.clone(),
        };
        active.cancellation.request();
        assert!(cancellation.is_requested());
    }

    #[test]
    fn terminal_events_are_scoped_to_the_active_operation() {
        let id = crate::runtime::OperationId(4);
        assert!(terminal_event(&RuntimeEvent::Cancelled(id), id));
        assert!(!terminal_event(
            &RuntimeEvent::Completed(
                crate::runtime::OperationId(5),
                Box::new(RuntimeValue::Status("done".into()))
            ),
            id
        ));
    }

    #[test]
    fn run_session_recovers_after_startup_and_accepts_next_command() {
        let runtime = ScriptRuntime::new();
        runtime.queue(completed(1, RuntimeValue::Status("ready".into())));
        runtime.queue(completed(2, RuntimeValue::Status("second".into())));
        let mut dialogs = ScriptDialogs {
            confirms: VecDeque::new(),
            selections: VecDeque::new(),
            seen: Vec::new(),
        };
        session(
            vec![Ok("status".into()), Ok("exit".into())],
            &runtime,
            &mut dialogs,
        );
        assert_eq!(
            &runtime.requests.lock().unwrap()[..],
            &[RuntimeRequest::ProjectStatus, RuntimeRequest::ProjectStatus]
        );
    }

    #[test]
    fn run_session_confirmation_yes_submits_exactly_one_cancel() {
        let runtime = ScriptRuntime::new();
        runtime.queue(completed(1, RuntimeValue::Status("ready".into())));
        runtime.queue(completed(2, RuntimeValue::Cancelled(true)));
        let mut dialogs = ScriptDialogs {
            confirms: VecDeque::from([true]),
            selections: VecDeque::new(),
            seen: Vec::new(),
        };
        session(
            vec![Ok("cancel T-0001".into()), Ok("exit".into())],
            &runtime,
            &mut dialogs,
        );
        assert_eq!(
            runtime
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| matches!(r, RuntimeRequest::CancelTask(_)))
                .count(),
            1
        );
    }

    #[test]
    fn run_session_confirmation_no_submits_no_cancel_and_shuts_down_on_eof() {
        let runtime = ScriptRuntime::new();
        runtime.queue(completed(1, RuntimeValue::Status("ready".into())));
        let mut dialogs = ScriptDialogs {
            confirms: VecDeque::from([false]),
            selections: VecDeque::new(),
            seen: Vec::new(),
        };
        session(
            vec![Ok("cancel T-0001".into()), Err(ReadlineError::Eof)],
            &runtime,
            &mut dialogs,
        );
        assert!(
            !runtime
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|r| matches!(r, RuntimeRequest::CancelTask(_)))
        );
    }

    #[test]
    fn run_session_routes_dispatch_to_selected_agent_and_remains_usable() {
        let runtime = ScriptRuntime::new();
        runtime.queue(completed(1, RuntimeValue::Status("ready".into())));
        runtime.queue(completed(
            2,
            RuntimeValue::AgentCandidates {
                task_id: "T-0001".into(),
                agents: vec!["codex-main".into(), "codex-secondary".into()],
            },
        ));
        runtime.queue(completed(3, RuntimeValue::Status("dispatched".into())));
        runtime.queue(completed(4, RuntimeValue::Status("still usable".into())));
        let mut dialogs = dialogs();
        dialogs.selections.push_back(1);

        session(
            vec![
                Ok("dispatch T-0001".into()),
                Ok("status".into()),
                Ok("exit".into()),
            ],
            &runtime,
            &mut dialogs,
        );

        assert_eq!(
            &runtime.requests.lock().unwrap()[..],
            &[
                RuntimeRequest::ProjectStatus,
                RuntimeRequest::DispatchCandidates("T-0001".into()),
                RuntimeRequest::Dispatch {
                    task_id: "T-0001".into(),
                    agent_id: Some("codex-secondary".into()),
                },
                RuntimeRequest::ProjectStatus,
            ]
        );
        assert_eq!(
            dialogs.seen,
            vec![vec![
                "codex-main".to_string(),
                "codex-secondary".to_string()
            ]]
        );
    }

    #[test]
    fn run_session_prints_async_event_before_completion_and_accepts_more_input() {
        let runtime = ScriptRuntime::new();
        runtime.queue(completed(1, RuntimeValue::Status("ready".into())));
        let events = runtime.events.clone();
        let output = Arc::new((Mutex::new(Vec::<String>::new()), std::sync::Condvar::new()));
        let observed = output.clone();
        let events_after = events.clone();
        let input = CoordinatedInput(VecDeque::from([
            Box::new(|| Ok("status".into())) as InputAction,
            Box::new(move || {
                events.lock().unwrap().extend([
                    RuntimeEvent::Lifecycle(crate::runtime::OperationId(2), lifecycle("working")),
                    completed(2, RuntimeValue::Status("operation complete".into())),
                ]);
                let (lines, emitted) = &*observed;
                let mut lines = lines.lock().unwrap();
                while !lines.iter().any(|line| line.contains("operation complete")) {
                    lines = emitted.wait(lines).unwrap();
                }
                Ok("tasks".into())
            }) as InputAction,
            Box::new(move || {
                events_after
                    .lock()
                    .unwrap()
                    .push_back(completed(3, RuntimeValue::Tasks(Vec::new())));
                Ok("exit".into())
            }) as InputAction,
        ]));
        let mut input = input;
        run_session(
            &runtime,
            &mut input,
            Arc::new(Mutex::new(SignallingCapture(output.clone()))),
            &mut dialogs(),
        )
        .unwrap();
        let lines = output.0.lock().unwrap();
        let progress = lines
            .iter()
            .position(|line| line.contains("worker: working"))
            .unwrap();
        let completion = lines
            .iter()
            .position(|line| line.contains("operation complete"))
            .unwrap();
        assert!(progress < completion);
        assert!(
            runtime
                .requests
                .lock()
                .unwrap()
                .contains(&RuntimeRequest::Tasks)
        );
    }

    #[test]
    fn run_session_ctrl_c_cancels_active_operation_and_recovers() {
        let runtime = ScriptRuntime::new();
        runtime.queue(completed(1, RuntimeValue::Status("ready".into())));
        let cancellations = runtime.cancellations.clone();
        let cancellations_after = cancellations.clone();
        let events = runtime.events.clone();
        let events_after = events.clone();
        let mut input = CoordinatedInput(VecDeque::from([
            Box::new(|| Ok("dispatch T-0001 codex-main".into())) as InputAction,
            Box::new(move || {
                let (items, submitted) = &*cancellations;
                let mut items = items.lock().unwrap();
                while items.len() < 2 {
                    items = submitted.wait(items).unwrap();
                }
                Err(ReadlineError::Interrupted)
            }) as InputAction,
            Box::new(move || {
                let (items, _) = &*cancellations_after;
                let items = items.lock().unwrap();
                assert!(items[1].is_requested());
                events
                    .lock()
                    .unwrap()
                    .push_back(RuntimeEvent::Cancelled(crate::runtime::OperationId(2)));
                Ok("status".into())
            }) as InputAction,
            Box::new(move || {
                events_after
                    .lock()
                    .unwrap()
                    .push_back(completed(3, RuntimeValue::Status("recovered".into())));
                Ok("exit".into())
            }) as InputAction,
        ]));
        session_with_input(&runtime, &mut input, &mut dialogs());
        assert_eq!(
            runtime.requests.lock().unwrap().last(),
            Some(&RuntimeRequest::ProjectStatus)
        );
    }

    #[test]
    fn run_session_ctrl_c_during_candidate_lookup_cancels_and_recovers() {
        let runtime = ScriptRuntime::new();
        runtime.queue(completed(1, RuntimeValue::Status("ready".into())));
        let cancellations = runtime.cancellations.clone();
        let cancellations_after = cancellations.clone();
        let events = runtime.events.clone();
        let events_after = events.clone();
        let mut input = CoordinatedInput(VecDeque::from([
            Box::new(|| Ok("dispatch T-0001".into())) as InputAction,
            Box::new(move || {
                let (items, submitted) = &*cancellations;
                let mut items = items.lock().unwrap();
                while items.len() < 2 {
                    items = submitted.wait(items).unwrap();
                }
                Err(ReadlineError::Interrupted)
            }) as InputAction,
            Box::new(move || {
                let (items, _) = &*cancellations_after;
                assert!(items.lock().unwrap()[1].is_requested());
                events
                    .lock()
                    .unwrap()
                    .push_back(RuntimeEvent::Cancelled(crate::runtime::OperationId(2)));
                Ok(String::new())
            }) as InputAction,
            Box::new(move || {
                events_after
                    .lock()
                    .unwrap()
                    .push_back(completed(3, RuntimeValue::Status("recovered".into())));
                Ok("status".into())
            }) as InputAction,
            Box::new(|| Ok("exit".into())) as InputAction,
        ]));
        session_with_input(&runtime, &mut input, &mut dialogs());
        assert_eq!(
            runtime.requests.lock().unwrap().last(),
            Some(&RuntimeRequest::ProjectStatus)
        );
    }

    #[test]
    fn run_session_readline_error_after_event_thread_starts_shuts_down() {
        let runtime = ScriptRuntime::new();
        runtime.queue(completed(1, RuntimeValue::Status("ready".into())));
        let result = run_session(
            &runtime,
            &mut ScriptInput(VecDeque::from([Err(ReadlineError::Io(
                std::io::Error::other("input failed"),
            ))])),
            Arc::new(Mutex::new(Capture::default())),
            &mut dialogs(),
        );
        let error = result.unwrap_err().to_string();
        assert!(error.contains("input failed"));
    }

    fn session_with_input(
        runtime: &ScriptRuntime,
        input: &mut CoordinatedInput,
        dialogs: &mut ScriptDialogs,
    ) -> Vec<String> {
        let output = Arc::new(Mutex::new(Vec::new()));
        run_session(
            runtime,
            input,
            Arc::new(Mutex::new(Capture(output.clone()))),
            dialogs,
        )
        .unwrap();
        Arc::try_unwrap(output).unwrap().into_inner().unwrap()
    }

    fn assert_candidate_terminal_recovery(event: RuntimeEvent, expected: &str) {
        let runtime = ScriptRuntime::new();
        runtime.queue(completed(1, RuntimeValue::Status("ready".into())));
        runtime.queue(event);
        runtime.queue(completed(3, RuntimeValue::Status("recovered".into())));
        let output = session(
            vec![
                Ok("dispatch T-0001".into()),
                Ok("status".into()),
                Ok("exit".into()),
            ],
            &runtime,
            &mut dialogs(),
        );
        assert!(
            output
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.contains(expected))
        );
        assert_eq!(
            runtime.requests.lock().unwrap().last(),
            Some(&RuntimeRequest::ProjectStatus)
        );
    }

    #[test]
    fn run_session_recovers_when_dispatch_candidates_fail() {
        assert_candidate_terminal_recovery(
            RuntimeEvent::Failed(
                crate::runtime::OperationId(2),
                "candidate lookup failed".into(),
            ),
            "candidate lookup failed",
        );
    }

    #[test]
    fn run_session_recovers_when_dispatch_candidates_are_cancelled() {
        assert_candidate_terminal_recovery(
            RuntimeEvent::Cancelled(crate::runtime::OperationId(2)),
            "operation cancelled",
        );
    }

    fn assert_explicit_shutdown(command: &str) {
        let runtime = ScriptRuntime::new();
        runtime.queue(completed(1, RuntimeValue::Status("ready".into())));
        session(vec![Ok(command.into())], &runtime, &mut dialogs());
        assert_eq!(
            &runtime.requests.lock().unwrap()[..],
            &[RuntimeRequest::ProjectStatus]
        );
    }

    #[test]
    fn run_session_exit_shuts_down_and_joins_event_pump() {
        assert_explicit_shutdown("exit");
    }

    #[test]
    fn run_session_quit_shuts_down_and_joins_event_pump() {
        assert_explicit_shutdown("quit");
    }
}
