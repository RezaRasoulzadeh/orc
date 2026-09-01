use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::operations::{TaskOperationsDetail, TaskOperationsSummary, ValidationState};
use crate::self_hosting::SelfHostingReadinessState;

use super::state::{Screen, TuiState};

const ACCENT: Color = Color::Cyan;

pub fn draw(frame: &mut Frame<'_>, state: &mut TuiState) {
    let area = frame.area();
    if area.width < 32 || area.height < 8 {
        frame.render_widget(
            Paragraph::new("Orc TUI needs at least 32x8. Resize or press Esc to exit.")
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(area);
    draw_header(frame, vertical[0], state);
    match state.screen {
        Screen::Queue => draw_queue(frame, vertical[1], state),
        Screen::Detail => draw_detail(frame, vertical[1], state),
    }
    draw_footer(frame, vertical[2], state);

    if let Some(input) = &state.revision_input {
        draw_revision_prompt(frame, area, input);
    }
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let readiness = match state.self_hosting.state {
        SelfHostingReadinessState::NotApplicable => "external",
        SelfHostingReadinessState::Ready => "self-hosting ready",
        SelfHostingReadinessState::Blocked => "self-hosting BLOCKED",
    };
    let text = Line::from(vec![
        Span::styled(
            " Orc ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("{}  |  {}", state.project_name, readiness)),
    ]);
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::BOTTOM)),
        area,
    );
}

fn draw_queue(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let panes = if area.width >= 64 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
            .split(area)
    };

    let items = if state.tasks.is_empty() {
        vec![ListItem::new("No tasks. Press r to refresh.")]
    } else {
        state
            .tasks
            .iter()
            .enumerate()
            .map(|(index, task)| {
                let prefix = if state.selected == Some(index) {
                    ">"
                } else {
                    " "
                };
                let blocker = if task.actionable_blocker_count > 0 {
                    format!(" !{}", task.actionable_blocker_count)
                } else if let Some(queue) = state.queue.find_item(&task.task_id) {
                    if !queue.waiting_on.is_empty() {
                        format!(" wait:{}", queue.waiting_on.len())
                    } else if !queue.blocking_reasons.is_empty() {
                        " blocked".into()
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };
                let text = format!(
                    "{prefix} {}  {:<18} {:?}{blocker}  {}",
                    task.task_id, task.lifecycle, task.priority, task.title
                );
                let style = if state.selected == Some(index) {
                    Style::default().fg(Color::Black).bg(ACCENT)
                } else {
                    Style::default()
                };
                ListItem::new(text).style(style)
            })
            .collect()
    };
    frame.render_widget(
        List::new(items).block(Block::default().title(" Tasks ").borders(Borders::ALL)),
        panes[0],
    );

    let preview = state
        .selected_task()
        .map(summary_lines)
        .unwrap_or_else(|| vec![Line::from("The queue is empty.")]);
    frame.render_widget(
        Paragraph::new(preview)
            .block(
                Block::default()
                    .title(" Selected task ")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        panes[1],
    );
}

fn draw_detail(frame: &mut Frame<'_>, area: Rect, state: &mut TuiState) {
    let lines = state
        .detail
        .as_ref()
        .map(|detail| detail_lines(detail, state))
        .unwrap_or_else(|| {
            vec![Line::from(
                "Task detail is unavailable; press Esc and refresh.",
            )]
        });
    let content_width = area.width.saturating_sub(2).max(1) as usize;
    let content_height = lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(content_width))
        .sum::<usize>();
    let paragraph = Paragraph::new(Text::from(lines))
        .block(
            Block::default()
                .title(" Task detail ")
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: false });
    state.set_detail_bounds(content_height, area.height.saturating_sub(2) as usize);
    let paragraph = paragraph.scroll((state.detail_scroll.min(u16::MAX as usize) as u16, 0));
    frame.render_widget(paragraph, area);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, state: &TuiState) {
    let help = match state.screen {
        Screen::Queue => "Up/Down or j/k navigate  Enter details  r refresh  Esc/q quit",
        Screen::Detail => "Up/Down or j/k scroll  r refresh  Esc queue",
    };
    let action = state
        .available_action()
        .map(|action| format!("  {} {}", action.key(), action.label()))
        .unwrap_or_default();
    let message = state
        .running
        .map(|action| format!("Running {}...", action.label()))
        .or_else(|| state.message.clone());
    let line = message.map_or_else(
        || format!(" {help}{action}"),
        |message| format!(" {message}  |  {help}{action}"),
    );
    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::TOP)),
        area,
    );
}

fn draw_revision_prompt(frame: &mut Frame<'_>, area: Rect, input: &str) {
    let width = area.width.saturating_sub(8).clamp(24, 72);
    let height = 5.min(area.height);
    let prompt = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, prompt);
    frame.render_widget(
        Paragraph::new(input)
            .block(
                Block::default()
                    .title(" Revision feedback (Enter submit, Esc cancel) ")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        prompt,
    );
}

fn summary_lines(task: &TaskOperationsSummary) -> Vec<Line<'static>> {
    let mut lines = vec![
        heading(&format!("{}  {}", task.task_id, task.title)),
        field("Status", task.lifecycle.to_string()),
        field("Priority", format!("{:?}", task.priority)),
        field("Role", task.role.clone()),
        field("Next", format!("{:?}", task.next_step)),
        Line::from(""),
        field("Objective", task.objective.clone()),
        field("Validation", format!("{:?}", task.validation.state)),
    ];
    if let Some(run) = task.current_run.as_ref().or(task.latest_run.as_ref()) {
        lines.push(field("Agent", run.agent.clone()));
        lines.push(field("Run", format!("{} ({})", run.id, run.status)));
    }
    if let Some(resolution) = &task.latest_resolution {
        lines.push(field(
            "Economy",
            format!(
                "{} / {} / {}",
                resolution.tier.as_str(),
                resolution.model.as_deref().unwrap_or("provider default"),
                resolution
                    .effort
                    .map_or_else(|| "default effort".into(), |effort| format!("{effort:?}"))
            ),
        ));
    }
    if task.actionable_blocker_count > 0 {
        lines.push(field("Blockers", task.actionable_blocker_count.to_string()));
    }
    lines
}

fn detail_lines(detail: &TaskOperationsDetail, state: &TuiState) -> Vec<Line<'static>> {
    let task = &detail.summary;
    let mut lines = summary_lines(task);
    lines.extend([Line::from(""), heading("Dependencies and blockers")]);
    if let Some(queue) = &detail.queue {
        lines.push(field(
            "Dependencies",
            if queue.dependencies.is_empty() {
                "none".into()
            } else {
                queue
                    .dependencies
                    .iter()
                    .map(|dependency| format!("{} [{:?}]", dependency.task_id, dependency.status))
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        ));
        lines.push(field(
            "Waiting on",
            if queue.waiting_on.is_empty() {
                "none".into()
            } else {
                queue.waiting_on.join(", ")
            },
        ));
        for reason in &queue.blocking_reasons {
            lines.push(field("Blocked", reason.to_string()));
        }
        if let Some(agent) = &queue.recommended_agent {
            lines.push(field("Recommended agent", agent.clone()));
        }
    }
    for blocker in &detail.blockers {
        lines.push(field(
            if blocker.actionable {
                "Blocker"
            } else {
                "Resolved"
            },
            format!("{}: {}", blocker.id, blocker.summary),
        ));
    }

    lines.extend([Line::from(""), heading("Validation and review")]);
    lines.push(field(
        "Validation",
        validation_text(task.validation.state, task.validation.is_current),
    ));
    for command in &task.validation.selected_commands {
        lines.push(field(
            "Command",
            format!("{} [{:?}]", command.command, command.passed),
        ));
    }
    lines.push(field(
        "Review",
        format!(
            "{}; criteria {}/{} satisfied; {} actionable blocker(s)",
            task.review.verdict.as_deref().unwrap_or("not run"),
            task.review.satisfied_criteria,
            task.review.total_criteria,
            task.review.actionable_blockers
        ),
    ));
    for criterion in &detail.review_criteria {
        lines.push(field(
            "Criterion",
            format!(
                "{} [{:?}] {}",
                criterion.criterion_id, criterion.status, criterion.criterion
            ),
        ));
        lines.push(field("Rationale", criterion.rationale.clone()));
        for evidence in &criterion.evidence {
            lines.push(field(
                "Evidence",
                format!("{:?}: {}", evidence.kind, evidence.reference),
            ));
        }
    }

    if state.self_hosting.recognized {
        lines.extend([Line::from(""), heading("Self-hosting readiness")]);
        lines.push(field("State", format!("{:?}", state.self_hosting.state)));
        for guard in &state.self_hosting.blocking_guards {
            lines.push(field("Guard", guard.clone()));
        }
    }
    lines
}

fn validation_text(state: ValidationState, current: Option<bool>) -> String {
    match current {
        Some(true) => format!("{state:?}, current for exact worktree/diff"),
        Some(false) => format!("{state:?}, stale for current worktree/diff"),
        None => format!("{state:?}"),
    }
}

fn heading(value: &str) -> Line<'static> {
    Line::from(Span::styled(
        value.to_owned(),
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    ))
}

fn field(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label}: "),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(value),
    ])
}
