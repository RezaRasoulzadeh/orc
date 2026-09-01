use crate::app::{CancelError, OrcApp};
use crate::storage::Database;
use crate::task::{CreateTaskInput, TaskPriority, TaskScopeMode};
use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum TaskCommand {
    /// Create one task directly in the current project.
    Create {
        title: String,
        objective: String,
        #[arg(long, default_value = "developer")]
        role: String,
        #[arg(long, value_parser = ["low", "normal", "high", "critical"], default_value = "normal")]
        priority: String,
        #[arg(long, value_delimiter = ',')]
        capability: Vec<String>,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, value_delimiter = ',')]
        context: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        expect: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        depends_on: Vec<String>,
    },
    List,
    /// Irreversibly delete a task and its persisted state.
    Purge {
        task_id: String,
        #[arg(long)]
        force: bool,
    },
    /// Recover an interrupted active task or failed blocked task and return it to the queue.
    Requeue {
        task_id: String,
    },
    /// Acknowledge a non-convergence replan requirement after inspection.
    Unblock {
        task_id: String,
    },
    /// Display task details
    Show {
        /// Task ID to display
        task_id: String,
    },
    /// Set required capabilities for a task
    Require {
        /// Task ID to configure
        task_id: String,
        /// Required capabilities (e.g. code terminal architecture review)
        #[arg(required = true, num_args = 1..)]
        capabilities: Vec<String>,
    },
    Scope {
        task_id: String,
        mode: String,
    },
    ContextAdd {
        task_id: String,
        paths: Vec<String>,
    },
    ContextClear {
        task_id: String,
    },
    ExpectChange {
        task_id: String,
        paths: Vec<String>,
    },
    ExpectClear {
        task_id: String,
    },
    /// Show the diff for a task worktree
    Diff {
        /// Task ID
        task_id: String,
    },
    /// Show worktree information for a task
    Worktree {
        /// Task ID
        task_id: String,
    },
    /// Accept a reviewed task and integrate its branch.
    Accept {
        task_id: String,
    },
    /// Reject a reviewed task while preserving its worktree.
    Reject {
        task_id: String,
        reason: Option<String>,
    },
    /// Cancel a task while preserving its worktree.
    Cancel {
        task_id: String,
        reason: Option<String>,
    },
    /// Add a dependency to a task
    Depend {
        /// Task ID that depends on another task
        task_id: String,
        /// Task ID that must be completed first
        dependency_id: String,
    },
    /// Remove a dependency from a task
    Undepend {
        /// Task ID that depends on another task
        task_id: String,
        /// Dependency task ID to remove
        dependency_id: String,
    },
}

pub fn run(command: TaskCommand, db_path: &str) -> Result<()> {
    match command {
        TaskCommand::Create {
            title,
            objective,
            role,
            priority,
            capability,
            scope,
            context,
            expect,
            depends_on,
        } => {
            let priority = match priority.as_str() {
                "low" => TaskPriority::Low,
                "normal" => TaskPriority::Normal,
                "high" => TaskPriority::High,
                "critical" => TaskPriority::Critical,
                _ => unreachable!(),
            };
            let scope_mode = scope
                .map(|value| {
                    TaskScopeMode::parse(&value)
                        .ok_or_else(|| anyhow::anyhow!("invalid scope mode: {value}"))
                })
                .transpose()?;
            let id = OrcApp::open_global(db_path, ".")?.create_task(CreateTaskInput {
                title,
                objective,
                role,
                priority,
                required_capabilities: capability,
                scope_mode,
                context_files: context,
                expected_changes: expect,
                dependencies: depends_on,
            })?;
            println!("Created task {id}");
        }
        TaskCommand::List => match OrcApp::open_global(db_path, ".") {
            Ok(app) => {
                let tasks = app.task_operation_summaries()?;
                for task in tasks {
                    println!("{}\t{}\t{}", task.task_id, task.lifecycle, task.title);
                }
            }
            Err(_) => {
                eprintln!("No DB found. Run `orc init` to initialize repository state.");
            }
        },
        TaskCommand::Purge { task_id, force } => {
            OrcApp::open_global(db_path, ".")?.purge_task(&task_id, force)?;
            println!("Purged task {}", task_id);
        }
        TaskCommand::Requeue { task_id } => {
            OrcApp::open_global(db_path, ".")?.requeue(&task_id)?;
            println!("Requeued task {task_id}");
        }
        TaskCommand::Unblock { task_id } => {
            OrcApp::open_global(db_path, ".")?.unblock_non_convergence(&task_id)?;
            println!("Acknowledged non-convergence condition for task {task_id}");
        }
        TaskCommand::Show { task_id } => match OrcApp::open_global(db_path, ".") {
            Ok(app) => match app.task_operations(&task_id)? {
                Some(detail) => {
                    let task = &detail.task;
                    println!("ID:           {}", task.id);
                    println!("Title:        {}", task.title);
                    println!("Objective:    {}", task.objective);
                    println!("Role:         {}", task.role);
                    println!("Priority:     {:?}", task.priority);
                    println!("Status:       {}", task.status);
                    println!(
                        "Effort:       {}",
                        task.reasoning_effort
                            .map(|effort| effort.as_str())
                            .unwrap_or("legacy/unspecified")
                    );
                    if let Some(reason) = &task.effort_reason {
                        println!("Effort reason: {}", reason);
                    }
                    if !task.risk_factors.is_empty() {
                        println!("Risk factors:  {:?}", task.risk_factors);
                    }
                    if let Some(condition) = &detail.execution_condition {
                        println!("Execution condition: {}", condition.kind);
                        println!("Condition details:  {}", condition.details);
                    }
                    if let Some(reason) = &task.cancellation_reason {
                        println!("Cancellation: {}", reason);
                    }
                    println!("Capabilities: {}", task.required_capabilities().join(", "));
                    println!(
                        "Scope: {}",
                        task.scope_mode
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "none".into())
                    );
                    println!("Context files:");
                    for path in &task.context_files {
                        println!("  {path}");
                    }
                    println!("Expected changes:");
                    for path in &task.expected_changes {
                        println!("  {path}");
                    }
                    let deps = detail
                        .queue
                        .as_ref()
                        .map(|entry| {
                            entry
                                .dependencies
                                .iter()
                                .map(|dependency| dependency.task_id.clone())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    if deps.is_empty() {
                        println!("Dependencies: none");
                    } else {
                        println!("Dependencies: {}", deps.join(", "));
                    }
                    println!("Operational phase: {}", detail.summary.phase);
                    println!("Next step: {:?}", detail.summary.next_step);
                    println!("Validation: {:?}", detail.summary.validation.state);
                    println!(
                        "Review:       {}",
                        detail
                            .summary
                            .review
                            .verdict
                            .as_deref()
                            .unwrap_or("not run")
                    );
                    println!(
                        "Criteria:     {} satisfied / {} violated / {} insufficient ({} total)",
                        detail.summary.review.satisfied_criteria,
                        detail.summary.review.violated_criteria,
                        detail.summary.review.insufficient_evidence_criteria,
                        detail.summary.review.total_criteria,
                    );
                    for criterion in &detail.review_criteria {
                        println!(
                            "  {} [{:?}] {}",
                            criterion.criterion_id, criterion.status, criterion.criterion
                        );
                        println!("    {}", criterion.rationale);
                        for evidence in &criterion.evidence {
                            println!(
                                "    evidence {:?} {}: {}",
                                evidence.kind, evidence.reference, evidence.explanation
                            );
                        }
                    }
                    if let Some(resolution) = &detail.summary.latest_resolution {
                        println!(
                            "Latest resolution: agent={} model={} effort={} tier={} source={}",
                            resolution.agent.as_deref().unwrap_or("unknown"),
                            resolution.model.as_deref().unwrap_or("unknown"),
                            resolution
                                .effort
                                .map(|effort| effort.as_str())
                                .unwrap_or("unknown"),
                            resolution.tier.as_str(),
                            resolution.source.as_deref().unwrap_or("legacy/unknown")
                        );
                    }
                    if !detail.resolutions.is_empty() {
                        println!("Provider invocations:");
                        for invocation in &detail.resolutions {
                            let (packet_type, packet_bytes, known_bytes, truncated, session) =
                                invocation
                                    .context
                                    .as_ref()
                                    .map(|context| {
                                        (
                                            context.packet.packet_type.as_str(),
                                            context.packet.bytes.to_string(),
                                            context
                                                .context_sources
                                                .iter()
                                                .filter(|source| source.included)
                                                .filter_map(|source| source.bytes)
                                                .sum::<usize>()
                                                .to_string(),
                                            if context.packet.truncated {
                                                "yes"
                                            } else {
                                                "no"
                                            },
                                            format!("{:?}", context.session_state)
                                                .to_ascii_lowercase(),
                                        )
                                    })
                                    .unwrap_or((
                                        "unknown",
                                        "unknown".into(),
                                        "unknown".into(),
                                        "unknown",
                                        "unknown".into(),
                                    ));
                            println!(
                                "  {} action={} packet={} packet_bytes={} known_context_bytes={} truncated={} session={} input={} cached={} output={} attribution={}",
                                invocation.invocation_id,
                                invocation.action.as_deref().unwrap_or(&invocation.purpose),
                                packet_type,
                                packet_bytes,
                                known_bytes,
                                truncated,
                                session,
                                invocation
                                    .token_usage
                                    .input_tokens
                                    .map_or_else(|| "unknown".into(), |value| value.to_string()),
                                invocation
                                    .token_usage
                                    .cached_input_tokens
                                    .map_or_else(|| "unknown".into(), |value| value.to_string()),
                                invocation
                                    .token_usage
                                    .output_tokens
                                    .map_or_else(|| "unknown".into(), |value| value.to_string()),
                                invocation.context_attribution_status,
                            );
                        }
                    }
                }
                None => eprintln!("Task {} not found", task_id),
            },
            Err(_) => eprintln!("No DB found. Run `orc init` to initialize repository state."),
        },
        TaskCommand::Require {
            task_id,
            capabilities,
        } => {
            let app = OrcApp::open_global(db_path, ".")?;
            if !app.set_task_required_capabilities(&task_id, &capabilities)? {
                anyhow::bail!("task '{}' not found in DB", task_id);
            }
            println!(
                "Updated capabilities for task {}: {}",
                task_id,
                capabilities.join(", ")
            );
        }
        TaskCommand::Cancel { task_id, reason } => {
            let app = OrcApp::open_global(db_path, ".")?;
            if let Err(error) = app.cancel(&task_id, reason.as_deref()) {
                match error {
                    CancelError::Database(error) => return Err(error.into()),
                    CancelError::Invalid(_) => {
                        let db = Database::open_global(db_path).map_err(|e| anyhow::anyhow!(e))?;
                        let task = db.get_task(&task_id)?;
                        match task {
                            None => anyhow::bail!("task '{}' not found", task_id),
                            Some(task) if task.status == crate::task::TaskStatus::Done => {
                                anyhow::bail!("task '{}' is done and cannot be cancelled", task_id)
                            }
                            Some(_) => anyhow::bail!("task '{}' is already cancelled", task_id),
                        }
                    }
                }
            }
            println!("Cancelled task {}", task_id);
        }
        TaskCommand::Scope { task_id, mode } => {
            let scope = TaskScopeMode::parse(&mode)
                .ok_or_else(|| anyhow::anyhow!("invalid scope mode: {mode}"))?;
            if !OrcApp::open_global(db_path, ".")?.set_task_scope(&task_id, scope)? {
                anyhow::bail!("task '{task_id}' not found");
            }
        }
        TaskCommand::ContextAdd { task_id, paths } => {
            if !OrcApp::open_global(db_path, ".")?.add_task_context(&task_id, &paths)? {
                anyhow::bail!("task '{task_id}' not found");
            }
        }
        TaskCommand::ExpectChange { task_id, paths } => {
            if !OrcApp::open_global(db_path, ".")?.add_expected_changes(&task_id, &paths)? {
                anyhow::bail!("task '{task_id}' not found");
            }
        }
        TaskCommand::ContextClear { task_id } => {
            if !OrcApp::open_global(db_path, ".")?.clear_task_context(&task_id)? {
                anyhow::bail!("task '{task_id}' not found");
            }
        }
        TaskCommand::ExpectClear { task_id } => {
            if !OrcApp::open_global(db_path, ".")?.clear_expected_changes(&task_id)? {
                anyhow::bail!("task '{task_id}' not found");
            }
        }
        TaskCommand::Diff { task_id } => show_diff(db_path, &task_id)?,
        TaskCommand::Worktree { task_id } => show_worktree(db_path, &task_id)?,
        TaskCommand::Accept { task_id } => {
            OrcApp::open_global(db_path, ".")?.accept(&task_id)?;
            println!(
                "Accepted task {}; changes integrated and task marked done.",
                task_id
            );
        }
        TaskCommand::Reject { task_id, reason } => {
            OrcApp::open_global(db_path, ".")?.reject(&task_id, reason.as_deref())?;
            println!(
                "Rejected task {}; worktree preserved and task moved to ready.",
                task_id
            );
        }
        TaskCommand::Depend {
            task_id,
            dependency_id,
        } => {
            OrcApp::open_global(db_path, ".")?.add_dependency(&task_id, &dependency_id)?;
            println!("Added dependency: {} depends on {}", task_id, dependency_id);
        }
        TaskCommand::Undepend {
            task_id,
            dependency_id,
        } => {
            if !OrcApp::open_global(db_path, ".")?.remove_dependency(&task_id, &dependency_id)? {
                anyhow::bail!("dependency '{}' -> '{}' not found", task_id, dependency_id);
            }
            println!(
                "Removed dependency: {} no longer depends on {}",
                task_id, dependency_id
            );
        }
    }
    Ok(())
}

fn show_diff(db_path: &str, task_id: &str) -> Result<()> {
    match Database::open_global(db_path) {
        Ok(db) => match db
            .get_worktree_metadata(task_id)
            .map_err(|e| anyhow::anyhow!(e))?
        {
            Some((_branch, path)) => match crate::git::inspect_worktree(path, ".") {
                Ok(changes) => {
                    if changes.diff.is_empty() {
                        println!("No changes in worktree for task {}", task_id);
                    } else {
                        println!("{}", changes.diff);
                    }
                }
                Err(e) => eprintln!("Failed to show diff: {}", e),
            },
            None => eprintln!("No worktree found for task {}", task_id),
        },
        Err(_) => eprintln!("No DB found. Run `orc init` to initialize repository state."),
    }
    Ok(())
}

fn show_worktree(db_path: &str, task_id: &str) -> Result<()> {
    match Database::open_global(db_path) {
        Ok(db) => match db
            .get_worktree_metadata(task_id)
            .map_err(|e| anyhow::anyhow!(e))?
        {
            Some((branch, path)) => {
                println!("Task:     {}", task_id);
                println!("Branch:   {}", branch);
                println!("Worktree: {}", path);
            }
            None => eprintln!("No worktree found for task {}", task_id),
        },
        Err(_) => eprintln!("No DB found. Run `orc init` to initialize repository state."),
    }
    Ok(())
}
