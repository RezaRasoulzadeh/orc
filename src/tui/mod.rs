mod event;
mod state;
mod terminal;
mod ui;

use std::path::Path;

use anyhow::Result;

use crate::app::OrcApp;
use crate::automated::ActionOverrides;

use self::state::{Intent, LifecycleAction, Screen, TuiState};

pub fn run(database_path: impl AsRef<Path>, repository_path: impl AsRef<Path>) -> Result<()> {
    let app = OrcApp::open_global(database_path, repository_path)?;
    let project_name = app.operations().project_name()?;
    let snapshot = app.operations().snapshot()?;
    let mut state = TuiState::new(project_name, snapshot);

    terminal::install_panic_restore_hook();
    let mut session = terminal::TerminalSession::enter()?;
    loop {
        session
            .terminal()
            .draw(|frame| ui::draw(frame, &mut state))?;
        let Some(key) = event::next_key()? else {
            continue;
        };
        match state.handle_key(key) {
            Intent::None => {}
            Intent::Quit => break,
            Intent::Refresh => refresh(&app, &mut state),
            Intent::OpenDetail => open_detail(&app, &mut state),
            Intent::Run(LifecycleAction::Revise) => {
                state.revision_input = Some(String::new());
                state.message = Some("enter concise revision feedback".into());
            }
            Intent::Run(action) => run_action(&app, &mut state, action, None, session.terminal()),
            Intent::SubmitRevision(feedback) => run_action(
                &app,
                &mut state,
                LifecycleAction::Revise,
                Some(feedback),
                session.terminal(),
            ),
        }
    }
    Ok(())
}

fn refresh(app: &OrcApp, state: &mut TuiState) {
    let selected_id = state.selected_task_id().map(str::to_owned);
    let was_detail = state.screen == Screen::Detail;
    match app.operations().snapshot() {
        Ok(snapshot) => {
            state.refresh(snapshot);
            if was_detail
                && selected_id.as_deref() == state.selected_task_id()
                && let Some(task_id) = selected_id
            {
                match app.task_operations(&task_id) {
                    Ok(Some(detail)) => state.set_detail(detail),
                    Ok(None) => state.message = Some(format!("task {task_id} no longer exists")),
                    Err(error) => state.message = Some(short_error("refresh detail", &error)),
                }
            }
            if state.message.is_none() {
                state.message = Some("refreshed".into());
            }
        }
        Err(error) => state.message = Some(short_error("refresh", &error)),
    }
}

fn open_detail(app: &OrcApp, state: &mut TuiState) {
    let Some(task_id) = state.selected_task_id().map(str::to_owned) else {
        return;
    };
    match app.task_operations(&task_id) {
        Ok(Some(detail)) => state.set_detail(detail),
        Ok(None) => state.message = Some(format!("task {task_id} no longer exists")),
        Err(error) => state.message = Some(short_error("load detail", &error)),
    }
}

fn run_action(
    app: &OrcApp,
    state: &mut TuiState,
    action: LifecycleAction,
    feedback: Option<String>,
    terminal: &mut terminal::OrcTerminal,
) {
    if state.running.is_some() || state.available_action() != Some(action) {
        state.message = Some("action is no longer valid; refresh the task".into());
        return;
    }
    let Some(task_id) = state.selected_task_id().map(str::to_owned) else {
        return;
    };
    state.running = Some(action);
    state.message = None;
    if let Err(error) = terminal.draw(|frame| ui::draw(frame, state)) {
        state.running = None;
        state.message = Some(short_error("render running state", &error.into()));
        return;
    }

    let outcome = match action {
        LifecycleAction::Dispatch => app
            .dispatch(&task_id, None)
            .map(|summary| format!("dispatch finished: run {}", summary.run_status)),
        LifecycleAction::Review => app
            .automated_review(&task_id, &ActionOverrides::default())
            .map(|(_, review)| format!("review finished: {}", review.verdict)),
        LifecycleAction::Revise => revise(app, &task_id, feedback.as_deref().unwrap_or_default()),
        LifecycleAction::Accept => app.accept(&task_id).map(|()| "task accepted".to_string()),
    };
    state.running = None;
    state.revision_input = None;
    state.message = Some(match outcome {
        Ok(message) => message,
        Err(error) => short_error(action.label(), &error),
    });
    let action_message = state.message.take();
    refresh(app, state);
    state.message = action_message;
}

fn revise(app: &OrcApp, task_id: &str, feedback: &str) -> Result<String> {
    app.revise_with_previous_agent(task_id, feedback)?;
    Ok("revision finished".into())
}

fn short_error(context: &str, error: &anyhow::Error) -> String {
    let message = format!("{error:#}").replace(['\n', '\r'], " ");
    let message = message.chars().take(240).collect::<String>();
    format!("{context} failed: {message}")
}
