use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::operations::{
    OperationalNextStep, ProjectOperationsSnapshot, TaskOperationsDetail, TaskOperationsSummary,
};
use crate::queue::QueueReport;
use crate::self_hosting::SelfHostingReadiness;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Queue,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    Dispatch,
    Review,
    Revise,
    Accept,
}

impl LifecycleAction {
    pub const fn key(self) -> char {
        match self {
            Self::Dispatch => 'd',
            Self::Review => 'v',
            Self::Revise => 'e',
            Self::Accept => 'a',
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Dispatch => "dispatch",
            Self::Review => "review",
            Self::Revise => "revise",
            Self::Accept => "accept",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    None,
    Quit,
    Refresh,
    OpenDetail,
    Run(LifecycleAction),
    SubmitRevision(String),
}

#[derive(Debug)]
pub struct TuiState {
    pub project_name: String,
    pub self_hosting: SelfHostingReadiness,
    pub queue: QueueReport,
    pub tasks: Vec<TaskOperationsSummary>,
    pub selected: Option<usize>,
    pub screen: Screen,
    pub detail: Option<TaskOperationsDetail>,
    pub detail_scroll: usize,
    pub detail_max_scroll: usize,
    pub message: Option<String>,
    pub running: Option<LifecycleAction>,
    pub revision_input: Option<String>,
}

impl TuiState {
    pub fn new(project_name: Option<String>, snapshot: ProjectOperationsSnapshot) -> Self {
        Self::from_read_model(
            project_name,
            snapshot.self_hosting,
            snapshot.queue,
            snapshot.tasks,
        )
    }

    pub(crate) fn from_read_model(
        project_name: Option<String>,
        self_hosting: SelfHostingReadiness,
        queue: QueueReport,
        tasks: Vec<TaskOperationsSummary>,
    ) -> Self {
        let selected = (!tasks.is_empty()).then_some(0);
        Self {
            project_name: project_name.unwrap_or_else(|| "unnamed project".into()),
            self_hosting,
            queue,
            tasks,
            selected,
            screen: Screen::Queue,
            detail: None,
            detail_scroll: 0,
            detail_max_scroll: 0,
            message: None,
            running: None,
            revision_input: None,
        }
    }

    pub fn selected_task(&self) -> Option<&TaskOperationsSummary> {
        self.selected.and_then(|index| self.tasks.get(index))
    }

    pub fn selected_task_id(&self) -> Option<&str> {
        self.selected_task().map(|task| task.task_id.as_str())
    }

    pub fn move_down(&mut self) {
        if let Some(index) = self.selected {
            self.selected = Some((index + 1).min(self.tasks.len().saturating_sub(1)));
        }
    }

    pub fn move_up(&mut self) {
        if let Some(index) = self.selected {
            self.selected = Some(index.saturating_sub(1));
        }
    }

    pub fn refresh(&mut self, snapshot: ProjectOperationsSnapshot) {
        self.refresh_read_model(snapshot.self_hosting, snapshot.queue, snapshot.tasks);
    }

    pub(crate) fn refresh_read_model(
        &mut self,
        self_hosting: SelfHostingReadiness,
        queue: QueueReport,
        tasks: Vec<TaskOperationsSummary>,
    ) {
        let selected_id = self.selected_task_id().map(str::to_owned);
        let old_index = self.selected.unwrap_or_default();
        self.self_hosting = self_hosting;
        self.queue = queue;
        self.tasks = tasks;
        self.selected = if self.tasks.is_empty() {
            None
        } else if let Some(selected_id) = selected_id {
            self.tasks
                .iter()
                .position(|task| task.task_id == selected_id)
                .or_else(|| Some(old_index.min(self.tasks.len() - 1)))
        } else {
            Some(0)
        };
        if self
            .detail
            .as_ref()
            .is_some_and(|detail| Some(detail.summary.task_id.as_str()) != self.selected_task_id())
        {
            self.detail = None;
            self.screen = Screen::Queue;
            self.detail_scroll = 0;
        }
    }

    pub fn set_detail(&mut self, detail: TaskOperationsDetail) {
        self.detail = Some(detail);
        self.screen = Screen::Detail;
        self.detail_scroll = 0;
    }

    pub fn set_detail_bounds(&mut self, content_height: usize, viewport_height: usize) {
        self.detail_max_scroll = content_height.saturating_sub(viewport_height);
        self.detail_scroll = self.detail_scroll.min(self.detail_max_scroll);
    }

    pub fn available_action(&self) -> Option<LifecycleAction> {
        self.selected_task().and_then(|task| {
            action_for_next_step(task.next_step).filter(|_| self.running.is_none())
        })
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Intent {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return Intent::None;
        }
        if let Some(input) = self.revision_input.as_mut() {
            return match key.code {
                KeyCode::Esc => {
                    self.revision_input = None;
                    Intent::None
                }
                KeyCode::Enter => {
                    let feedback = self.revision_input.take().unwrap_or_default();
                    if feedback.trim().is_empty() {
                        self.message = Some("revision feedback cannot be empty".into());
                        Intent::None
                    } else {
                        Intent::SubmitRevision(feedback)
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                    Intent::None
                }
                KeyCode::Char(character)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    input.push(character);
                    Intent::None
                }
                _ => Intent::None,
            };
        }

        match (self.screen, key.code) {
            (Screen::Queue, KeyCode::Char('q') | KeyCode::Esc) => Intent::Quit,
            (_, KeyCode::Char('r')) => Intent::Refresh,
            (Screen::Queue, KeyCode::Down | KeyCode::Char('j')) => {
                self.move_down();
                Intent::None
            }
            (Screen::Queue, KeyCode::Up | KeyCode::Char('k')) => {
                self.move_up();
                Intent::None
            }
            (Screen::Queue, KeyCode::Enter) if self.selected.is_some() => Intent::OpenDetail,
            (Screen::Detail, KeyCode::Esc) => {
                self.screen = Screen::Queue;
                Intent::None
            }
            (Screen::Detail, KeyCode::Down | KeyCode::Char('j')) => {
                self.detail_scroll = (self.detail_scroll + 1).min(self.detail_max_scroll);
                Intent::None
            }
            (Screen::Detail, KeyCode::Up | KeyCode::Char('k')) => {
                self.detail_scroll = self.detail_scroll.saturating_sub(1);
                Intent::None
            }
            (_, KeyCode::Char(key)) => self
                .available_action()
                .filter(|action| action.key() == key)
                .map(Intent::Run)
                .unwrap_or(Intent::None),
            _ => Intent::None,
        }
    }
}

pub const fn action_for_next_step(next_step: OperationalNextStep) -> Option<LifecycleAction> {
    match next_step {
        OperationalNextStep::Dispatch => Some(LifecycleAction::Dispatch),
        OperationalNextStep::RunSemanticReview => Some(LifecycleAction::Review),
        OperationalNextStep::Revise => Some(LifecycleAction::Revise),
        OperationalNextStep::Accept => Some(LifecycleAction::Accept),
        OperationalNextStep::WaitForExecution
        | OperationalNextStep::ResolveBlocker
        | OperationalNextStep::SatisfyDependencies
        | OperationalNextStep::ConfigureEligibleAgent
        | OperationalNextStep::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::{ReviewOperationsSummary, TokenUsageSummary, ValidationSummary};
    use crate::queue::QueueCategory;
    use crate::self_hosting::SelfHostingReadinessState;
    use crate::task::{TaskPriority, TaskStatus};

    fn readiness() -> SelfHostingReadiness {
        SelfHostingReadiness {
            recognized: false,
            repository_id: None,
            state: SelfHostingReadinessState::NotApplicable,
            blocking_guards: Vec::new(),
        }
    }

    fn task(id: &str, next_step: OperationalNextStep) -> TaskOperationsSummary {
        TaskOperationsSummary {
            task_id: id.into(),
            title: format!("Task {id}"),
            objective: "Exercise the TUI state".into(),
            role: "developer".into(),
            priority: TaskPriority::Normal,
            lifecycle: TaskStatus::Ready,
            phase: QueueCategory::Ready,
            next_step,
            cancellation_reason: None,
            current_run: None,
            latest_run: None,
            validation: ValidationSummary::default(),
            review: ReviewOperationsSummary {
                run_id: None,
                verdict: None,
                timestamp: None,
                applies_to_current_change: None,
                ready_for_review: false,
                actionable_blockers: 0,
                unresolved_blockers: 0,
                regressed_blockers: 0,
                resolved_blockers: 0,
                total_criteria: 0,
                satisfied_criteria: 0,
                violated_criteria: 0,
                insufficient_evidence_criteria: 0,
            },
            actionable_blocker_count: 0,
            latest_resolution: None,
            token_usage: TokenUsageSummary::default(),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn constructs_state_from_read_model_and_handles_empty_queue() {
        let populated = TuiState::from_read_model(
            Some("ledger".into()),
            readiness(),
            QueueReport::default(),
            vec![task("T-0001", OperationalNextStep::Dispatch)],
        );
        assert_eq!(populated.project_name, "ledger");
        assert_eq!(populated.selected_task_id(), Some("T-0001"));

        let empty =
            TuiState::from_read_model(None, readiness(), QueueReport::default(), Vec::new());
        assert_eq!(empty.project_name, "unnamed project");
        assert_eq!(empty.selected, None);
        assert_eq!(empty.selected_task_id(), None);
    }

    #[test]
    fn selection_moves_within_bounds() {
        let mut state = TuiState::from_read_model(
            None,
            readiness(),
            QueueReport::default(),
            vec![
                task("T-0001", OperationalNextStep::Dispatch),
                task("T-0002", OperationalNextStep::Dispatch),
            ],
        );
        state.move_up();
        assert_eq!(state.selected, Some(0));
        state.move_down();
        state.move_down();
        assert_eq!(state.selected, Some(1));
    }

    #[test]
    fn refresh_preserves_task_identity_then_clamps_when_removed() {
        let mut state = TuiState::from_read_model(
            None,
            readiness(),
            QueueReport::default(),
            vec![
                task("T-0001", OperationalNextStep::Dispatch),
                task("T-0002", OperationalNextStep::Dispatch),
            ],
        );
        state.move_down();
        state.refresh_read_model(
            readiness(),
            QueueReport::default(),
            vec![
                task("T-0002", OperationalNextStep::Dispatch),
                task("T-0003", OperationalNextStep::Dispatch),
            ],
        );
        assert_eq!(state.selected_task_id(), Some("T-0002"));
        assert_eq!(state.selected, Some(0));

        state.refresh_read_model(
            readiness(),
            QueueReport::default(),
            vec![task("T-0003", OperationalNextStep::Dispatch)],
        );
        assert_eq!(state.selected, Some(0));
        assert_eq!(state.selected_task_id(), Some("T-0003"));
    }

    #[test]
    fn actions_follow_canonical_operational_next_step() {
        assert_eq!(
            action_for_next_step(OperationalNextStep::Dispatch),
            Some(LifecycleAction::Dispatch)
        );
        assert_eq!(
            action_for_next_step(OperationalNextStep::RunSemanticReview),
            Some(LifecycleAction::Review)
        );
        assert_eq!(
            action_for_next_step(OperationalNextStep::Revise),
            Some(LifecycleAction::Revise)
        );
        assert_eq!(
            action_for_next_step(OperationalNextStep::Accept),
            Some(LifecycleAction::Accept)
        );
        assert_eq!(
            action_for_next_step(OperationalNextStep::ResolveBlocker),
            None
        );
    }

    #[test]
    fn detail_scroll_is_bounded() {
        let mut state =
            TuiState::from_read_model(None, readiness(), QueueReport::default(), Vec::new());
        state.detail_scroll = 99;
        state.set_detail_bounds(30, 10);
        assert_eq!(state.detail_max_scroll, 20);
        assert_eq!(state.detail_scroll, 20);
        state.set_detail_bounds(5, 10);
        assert_eq!(state.detail_scroll, 0);
    }

    #[test]
    fn exit_and_navigation_transitions_are_explicit() {
        let mut state = TuiState::from_read_model(
            None,
            readiness(),
            QueueReport::default(),
            vec![task("T-0001", OperationalNextStep::Dispatch)],
        );
        assert_eq!(state.handle_key(key(KeyCode::Enter)), Intent::OpenDetail);
        state.screen = Screen::Detail;
        assert_eq!(state.handle_key(key(KeyCode::Char('q'))), Intent::None);
        assert_eq!(state.handle_key(key(KeyCode::Esc)), Intent::None);
        assert_eq!(state.screen, Screen::Queue);
        assert_eq!(state.handle_key(key(KeyCode::Char('q'))), Intent::Quit);
    }
}
