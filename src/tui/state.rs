use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::operations::{
    OperationalNextStep, ProjectOperationsSnapshot, TaskOperationsDetail, TaskOperationsSummary,
};
use crate::queue::QueueReport;
use crate::self_hosting::SelfHostingReadiness;
use crate::task::{CreateTaskInput, TaskPriority, TaskScopeMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Queue,
    Detail,
    CreateTask,
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
    OpenTaskCreation,
    CancelTaskCreation,
    SubmitTaskCreation,
    Run(LifecycleAction),
    SubmitRevision(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskCreateField {
    Title,
    Objective,
    Role,
    Priority,
    Capabilities,
    Scope,
    ContextFiles,
    ExpectedChanges,
    Dependencies,
}

impl TaskCreateField {
    const ALL: [Self; 9] = [
        Self::Title,
        Self::Objective,
        Self::Role,
        Self::Priority,
        Self::Capabilities,
        Self::Scope,
        Self::ContextFiles,
        Self::ExpectedChanges,
        Self::Dependencies,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Title => "Title",
            Self::Objective => "Objective",
            Self::Role => "Role",
            Self::Priority => "Priority",
            Self::Capabilities => "Required capabilities",
            Self::Scope => "Scope",
            Self::ContextFiles => "Context files",
            Self::ExpectedChanges => "Expected changes",
            Self::Dependencies => "Dependencies",
        }
    }

    pub const fn all() -> [Self; 9] {
        Self::ALL
    }

    const fn index(self) -> usize {
        match self {
            Self::Title => 0,
            Self::Objective => 1,
            Self::Role => 2,
            Self::Priority => 3,
            Self::Capabilities => 4,
            Self::Scope => 5,
            Self::ContextFiles => 6,
            Self::ExpectedChanges => 7,
            Self::Dependencies => 8,
        }
    }

    fn from_index(index: usize) -> Self {
        Self::ALL[index.min(Self::ALL.len() - 1)]
    }

    const fn is_text(self) -> bool {
        !matches!(self, Self::Priority | Self::Scope)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCreateForm {
    pub active_field: TaskCreateField,
    pub title: String,
    pub objective: String,
    pub role: String,
    pub priority: TaskPriority,
    pub capabilities: String,
    pub scope: Option<TaskScopeMode>,
    pub context_files: String,
    pub expected_changes: String,
    pub dependencies: String,
    cursor: usize,
}

impl Default for TaskCreateForm {
    fn default() -> Self {
        Self {
            active_field: TaskCreateField::Title,
            title: String::new(),
            objective: String::new(),
            role: "developer".into(),
            priority: TaskPriority::Normal,
            capabilities: String::new(),
            scope: None,
            context_files: String::new(),
            expected_changes: String::new(),
            dependencies: String::new(),
            cursor: 0,
        }
    }
}

impl TaskCreateForm {
    pub const fn field_count() -> usize {
        TaskCreateField::ALL.len()
    }

    pub fn active_value(&self) -> String {
        match self.active_field {
            TaskCreateField::Title => self.title.clone(),
            TaskCreateField::Objective => self.objective.clone(),
            TaskCreateField::Role => self.role.clone(),
            TaskCreateField::Priority => format_priority(self.priority).into(),
            TaskCreateField::Capabilities => self.capabilities.clone(),
            TaskCreateField::Scope => format_scope(self.scope).into(),
            TaskCreateField::ContextFiles => self.context_files.clone(),
            TaskCreateField::ExpectedChanges => self.expected_changes.clone(),
            TaskCreateField::Dependencies => self.dependencies.clone(),
        }
    }

    pub fn active_cursor(&self) -> usize {
        self.cursor
    }

    pub fn set_active_field(&mut self, field: TaskCreateField) {
        self.active_field = field;
        self.cursor = self.active_text().map_or(0, str::len);
    }

    pub fn move_field(&mut self, offset: isize) {
        let index = self.active_field.index() as isize + offset;
        let index = index.clamp(0, (Self::field_count() - 1) as isize) as usize;
        self.set_active_field(TaskCreateField::from_index(index));
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Intent> {
        match key.code {
            KeyCode::Esc => Some(Intent::CancelTaskCreation),
            KeyCode::Tab => {
                self.move_field(1);
                None
            }
            KeyCode::BackTab => {
                self.move_field(-1);
                None
            }
            KeyCode::Enter => {
                if self.active_field == TaskCreateField::Dependencies {
                    None
                } else {
                    self.move_field(1);
                    None
                }
            }
            KeyCode::Left if !self.active_field.is_text() => {
                self.cycle_choice(-1);
                None
            }
            KeyCode::Right if !self.active_field.is_text() => {
                self.cycle_choice(1);
                None
            }
            KeyCode::Home if self.active_field.is_text() => {
                self.cursor = 0;
                None
            }
            KeyCode::End if self.active_field.is_text() => {
                self.cursor = self.active_text().map_or(0, str::len);
                None
            }
            KeyCode::Left if self.active_field.is_text() => {
                self.cursor =
                    previous_char_boundary(self.active_text().unwrap_or_default(), self.cursor);
                None
            }
            KeyCode::Right if self.active_field.is_text() => {
                self.cursor =
                    next_char_boundary(self.active_text().unwrap_or_default(), self.cursor);
                None
            }
            KeyCode::Backspace if self.active_field.is_text() => {
                self.delete_previous_character();
                None
            }
            KeyCode::Delete if self.active_field.is_text() => {
                self.delete_character();
                None
            }
            KeyCode::Char(character)
                if self.active_field.is_text()
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.insert_character(character);
                None
            }
            KeyCode::Char(' ') if !self.active_field.is_text() => {
                self.cycle_choice(1);
                None
            }
            _ => None,
        }
    }

    pub fn to_input(&self) -> CreateTaskInput {
        CreateTaskInput {
            title: self.title.clone(),
            objective: self.objective.clone(),
            role: self.role.clone(),
            priority: self.priority,
            required_capabilities: csv_values(&self.capabilities),
            scope_mode: self.scope,
            context_files: csv_values(&self.context_files),
            expected_changes: csv_values(&self.expected_changes),
            dependencies: csv_values(&self.dependencies),
        }
    }

    fn active_text(&self) -> Option<&str> {
        match self.active_field {
            TaskCreateField::Title => Some(&self.title),
            TaskCreateField::Objective => Some(&self.objective),
            TaskCreateField::Role => Some(&self.role),
            TaskCreateField::Capabilities => Some(&self.capabilities),
            TaskCreateField::ContextFiles => Some(&self.context_files),
            TaskCreateField::ExpectedChanges => Some(&self.expected_changes),
            TaskCreateField::Dependencies => Some(&self.dependencies),
            TaskCreateField::Priority | TaskCreateField::Scope => None,
        }
    }

    fn active_text_mut(&mut self) -> Option<&mut String> {
        match self.active_field {
            TaskCreateField::Title => Some(&mut self.title),
            TaskCreateField::Objective => Some(&mut self.objective),
            TaskCreateField::Role => Some(&mut self.role),
            TaskCreateField::Capabilities => Some(&mut self.capabilities),
            TaskCreateField::ContextFiles => Some(&mut self.context_files),
            TaskCreateField::ExpectedChanges => Some(&mut self.expected_changes),
            TaskCreateField::Dependencies => Some(&mut self.dependencies),
            TaskCreateField::Priority | TaskCreateField::Scope => None,
        }
    }

    fn insert_character(&mut self, character: char) {
        let cursor = self.cursor;
        if let Some(value) = self.active_text_mut() {
            value.insert(cursor, character);
            self.cursor += character.len_utf8();
        }
    }

    fn delete_previous_character(&mut self) {
        let previous = previous_char_boundary(self.active_text().unwrap_or_default(), self.cursor);
        if previous == self.cursor {
            return;
        }
        let cursor = self.cursor;
        if let Some(value) = self.active_text_mut() {
            value.drain(previous..cursor);
            self.cursor = previous;
        }
    }

    fn delete_character(&mut self) {
        let next = next_char_boundary(self.active_text().unwrap_or_default(), self.cursor);
        if next == self.cursor {
            return;
        }
        let cursor = self.cursor;
        if let Some(value) = self.active_text_mut() {
            value.drain(cursor..next);
        }
    }

    fn cycle_choice(&mut self, offset: isize) {
        match self.active_field {
            TaskCreateField::Priority => {
                const VALUES: [TaskPriority; 4] = [
                    TaskPriority::Low,
                    TaskPriority::Normal,
                    TaskPriority::High,
                    TaskPriority::Critical,
                ];
                let current = VALUES
                    .iter()
                    .position(|value| *value == self.priority)
                    .unwrap_or(1);
                let next = (current as isize + offset).rem_euclid(VALUES.len() as isize) as usize;
                self.priority = VALUES[next];
            }
            TaskCreateField::Scope => {
                const VALUES: [Option<TaskScopeMode>; 4] = [
                    None,
                    Some(TaskScopeMode::Focused),
                    Some(TaskScopeMode::Module),
                    Some(TaskScopeMode::Project),
                ];
                let current = VALUES
                    .iter()
                    .position(|value| *value == self.scope)
                    .unwrap_or(0);
                let next = (current as isize + offset).rem_euclid(VALUES.len() as isize) as usize;
                self.scope = VALUES[next];
            }
            _ => {}
        }
    }
}

fn csv_values(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

fn format_priority(priority: TaskPriority) -> &'static str {
    match priority {
        TaskPriority::Low => "low",
        TaskPriority::Normal => "normal",
        TaskPriority::High => "high",
        TaskPriority::Critical => "critical",
    }
}

fn format_scope(scope: Option<TaskScopeMode>) -> &'static str {
    match scope {
        None => "none",
        Some(TaskScopeMode::Focused) => "focused",
        Some(TaskScopeMode::Module) => "module",
        Some(TaskScopeMode::Project) => "project",
    }
}

fn previous_char_boundary(value: &str, cursor: usize) -> usize {
    value[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_char_boundary(value: &str, cursor: usize) -> usize {
    value[cursor..]
        .chars()
        .next()
        .map_or(cursor, |character| cursor + character.len_utf8())
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
    pub running_task_id: Option<String>,
    pub revision_input: Option<String>,
    pub task_creation: Option<TaskCreateForm>,
    pub creating: bool,
    pub confirmation: Option<LifecycleAction>,
    pending_task_creation: Option<CreateTaskInput>,
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
            running_task_id: None,
            revision_input: None,
            task_creation: None,
            creating: false,
            confirmation: None,
            pending_task_creation: None,
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

    pub fn open_task_creation(&mut self) {
        self.task_creation = Some(TaskCreateForm::default());
        self.confirmation = None;
        self.pending_task_creation = None;
        self.screen = Screen::CreateTask;
        self.message = None;
    }

    pub fn take_task_creation(&mut self) -> Option<CreateTaskInput> {
        self.pending_task_creation.take()
    }

    pub fn select_task_id(&mut self, task_id: &str) {
        if let Some(index) = self.tasks.iter().position(|task| task.task_id == task_id) {
            self.selected = Some(index);
            self.detail = None;
            self.screen = Screen::Queue;
        }
    }

    pub fn set_detail_bounds(&mut self, content_height: usize, viewport_height: usize) {
        self.detail_max_scroll = content_height.saturating_sub(viewport_height);
        self.detail_scroll = self.detail_scroll.min(self.detail_max_scroll);
    }

    pub fn available_action(&self) -> Option<LifecycleAction> {
        if self.screen == Screen::CreateTask {
            return None;
        }
        self.selected_task().and_then(|task| {
            action_for_next_step(task.next_step).filter(|_| self.running.is_none())
        })
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Intent {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return Intent::None;
        }
        if self.task_creation.is_some() {
            if let Some(intent) = self
                .task_creation
                .as_mut()
                .and_then(|form| form.handle_key(key))
            {
                if intent == Intent::CancelTaskCreation {
                    self.task_creation = None;
                    self.screen = Screen::Queue;
                    self.message = Some("task creation cancelled".into());
                }
                return intent;
            }
            if key.code == KeyCode::Enter
                && let Some(form) = self.task_creation.as_ref()
                && form.active_field == TaskCreateField::Dependencies
            {
                let input = form.to_input();
                self.pending_task_creation = Some(input);
                return Intent::SubmitTaskCreation;
            }
            return Intent::None;
        }
        if let Some(action) = self.confirmation {
            return match key.code {
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.confirmation = None;
                    self.message = Some("action cancelled".into());
                    Intent::None
                }
                KeyCode::Enter | KeyCode::Char('y') => {
                    self.confirmation = None;
                    Intent::Run(action)
                }
                _ => Intent::None,
            };
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
            (Screen::Queue | Screen::Detail, KeyCode::Char('n')) => Intent::OpenTaskCreation,
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
                .map(|action| {
                    if action == LifecycleAction::Accept {
                        self.confirmation = Some(action);
                        Intent::None
                    } else {
                        Intent::Run(action)
                    }
                })
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
    fn task_creation_form_maps_every_create_task_field() {
        let form = TaskCreateForm {
            title: "Add TUI lifecycle".into(),
            objective: "Make one normal task completable in the TUI".into(),
            role: "developer".into(),
            priority: TaskPriority::High,
            capabilities: "code, command_execution".into(),
            scope: Some(TaskScopeMode::Module),
            context_files: "src/tui/state.rs, src/tui/ui.rs".into(),
            expected_changes: "src/tui".into(),
            dependencies: "T-0001, T-0002".into(),
            ..TaskCreateForm::default()
        };

        let input = form.to_input();
        assert_eq!(input.title, "Add TUI lifecycle");
        assert_eq!(
            input.objective,
            "Make one normal task completable in the TUI"
        );
        assert_eq!(input.role, "developer");
        assert_eq!(input.priority, TaskPriority::High);
        assert_eq!(input.required_capabilities, ["code", "command_execution"]);
        assert_eq!(input.scope_mode, Some(TaskScopeMode::Module));
        assert_eq!(input.context_files, ["src/tui/state.rs", "src/tui/ui.rs"]);
        assert_eq!(input.expected_changes, ["src/tui"]);
        assert_eq!(input.dependencies, ["T-0001", "T-0002"]);
    }

    #[test]
    fn task_creation_editing_and_cancel_are_local_and_safe() {
        let mut state =
            TuiState::from_read_model(None, readiness(), QueueReport::default(), Vec::new());
        assert_eq!(
            state.handle_key(key(KeyCode::Char('n'))),
            Intent::OpenTaskCreation
        );
        state.open_task_creation();
        assert_eq!(state.screen, Screen::CreateTask);

        assert_eq!(state.handle_key(key(KeyCode::Char('x'))), Intent::None);
        assert_eq!(state.task_creation.as_ref().unwrap().title, "x");
        assert_eq!(state.handle_key(key(KeyCode::Backspace)), Intent::None);
        assert_eq!(state.task_creation.as_ref().unwrap().title, "");
        assert_eq!(state.handle_key(key(KeyCode::Tab)), Intent::None);
        assert_eq!(
            state.task_creation.as_ref().unwrap().active_field,
            TaskCreateField::Objective
        );

        assert_eq!(
            state.handle_key(key(KeyCode::Esc)),
            Intent::CancelTaskCreation
        );
        assert_eq!(state.screen, Screen::Queue);
        assert!(state.task_creation.is_none());
        assert!(state.take_task_creation().is_none());
    }

    #[test]
    fn completed_creation_form_emits_input_without_mutating_tasks() {
        let mut state =
            TuiState::from_read_model(None, readiness(), QueueReport::default(), Vec::new());
        state.open_task_creation();
        let form = state.task_creation.as_mut().unwrap();
        form.title = "New task".into();
        form.objective = "A valid objective".into();
        form.set_active_field(TaskCreateField::Dependencies);

        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            Intent::SubmitTaskCreation
        );
        assert!(state.tasks.is_empty());
        assert_eq!(state.take_task_creation().unwrap().title, "New task");
    }

    #[test]
    fn acceptance_requires_confirmation_and_illegal_actions_are_ignored() {
        let mut state = TuiState::from_read_model(
            None,
            readiness(),
            QueueReport::default(),
            vec![task("T-0001", OperationalNextStep::Accept)],
        );
        assert_eq!(state.handle_key(key(KeyCode::Char('a'))), Intent::None);
        assert_eq!(state.confirmation, Some(LifecycleAction::Accept));
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            Intent::Run(LifecycleAction::Accept)
        );
        assert!(state.confirmation.is_none());

        state.tasks[0] = task("T-0001", OperationalNextStep::WaitForExecution);
        assert_eq!(state.handle_key(key(KeyCode::Char('a'))), Intent::None);
        assert!(state.confirmation.is_none());
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
