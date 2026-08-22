use crate::app::{CancelError, OrcApp};
use crate::storage::Database;
use crate::task::TaskScopeMode;
use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum TaskCommand {
    List,
    /// Recover an interrupted active task and return it to the queue.
    Requeue {
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
        TaskCommand::List => match Database::open(db_path) {
            Ok(db) => {
                let tasks = db.list_tasks().map_err(|e| anyhow::anyhow!(e))?;
                for task in tasks {
                    println!("{}\t{}\t{}", task.id, task.status, task.title);
                }
            }
            Err(_) => {
                eprintln!("No DB found. Run `orc init` to initialize repository state.");
            }
        },
        TaskCommand::Requeue { task_id } => {
            OrcApp::open(db_path, ".")?.requeue(&task_id)?;
            println!("Requeued task {task_id}");
        }
        TaskCommand::Show { task_id } => match Database::open(db_path) {
            Ok(db) => match db.get_task(&task_id).map_err(|e| anyhow::anyhow!(e))? {
                Some(task) => {
                    println!("ID:           {}", task.id);
                    println!("Title:        {}", task.title);
                    println!("Objective:    {}", task.objective);
                    println!("Role:         {}", task.role);
                    println!("Priority:     {:?}", task.priority);
                    println!("Status:       {}", task.status);
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
                    let deps = db
                        .list_task_dependencies(&task_id)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    if deps.is_empty() {
                        println!("Dependencies: none");
                    } else {
                        println!("Dependencies: {}", deps.join(", "));
                    }
                }
                None => eprintln!("Task {} not found", task_id),
            },
            Err(_) => eprintln!("No DB found. Run `orc init` to initialize repository state."),
        },
        TaskCommand::Require {
            task_id,
            capabilities,
        } => match Database::open(db_path) {
            Ok(db) => {
                let changed = db
                    .set_task_required_capabilities(&task_id, &capabilities)
                    .map_err(|e| anyhow::anyhow!(e))?;
                if !changed {
                    eprintln!("Task {} not found", task_id);
                    anyhow::bail!("task '{}' not found in DB", task_id);
                }
                println!(
                    "Updated capabilities for task {}: {}",
                    task_id,
                    capabilities.join(", ")
                );
            }
            Err(_) => eprintln!("No DB found. Run `orc init` to initialize repository state."),
        },
        TaskCommand::Cancel { task_id, reason } => {
            let app = OrcApp::open(db_path, ".")?;
            if let Err(error) = app.cancel(&task_id, reason.as_deref()) {
                match error {
                    CancelError::Database(error) => return Err(error.into()),
                    CancelError::Invalid(_) => {
                        let db = Database::open(db_path).map_err(|e| anyhow::anyhow!(e))?;
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
            let db = Database::open(db_path).map_err(|e| anyhow::anyhow!(e))?;
            if !db
                .set_task_scope(&task_id, scope)
                .map_err(|e| anyhow::anyhow!(e))?
            {
                anyhow::bail!("task '{task_id}' not found");
            }
        }
        TaskCommand::ContextAdd { task_id, paths } => {
            let db = Database::open(db_path).map_err(|e| anyhow::anyhow!(e))?;
            let task = db
                .get_task(&task_id)
                .map_err(|e| anyhow::anyhow!(e))?
                .ok_or_else(|| anyhow::anyhow!("task '{task_id}' not found"))?;
            let mut values = task.context_files;
            values.extend(paths);
            db.set_task_context(&task_id, &values)?;
        }
        TaskCommand::ExpectChange { task_id, paths } => {
            let db = Database::open(db_path).map_err(|e| anyhow::anyhow!(e))?;
            let task = db
                .get_task(&task_id)
                .map_err(|e| anyhow::anyhow!(e))?
                .ok_or_else(|| anyhow::anyhow!("task '{task_id}' not found"))?;
            let mut values = task.expected_changes;
            values.extend(paths);
            db.set_task_expected_changes(&task_id, &values)?;
        }
        TaskCommand::ContextClear { task_id } => {
            let db = Database::open(db_path).map_err(|e| anyhow::anyhow!(e))?;
            db.set_task_context(&task_id, &Vec::new())?;
        }
        TaskCommand::ExpectClear { task_id } => {
            let db = Database::open(db_path).map_err(|e| anyhow::anyhow!(e))?;
            db.set_task_expected_changes(&task_id, &Vec::new())?;
        }
        TaskCommand::Diff { task_id } => show_diff(db_path, &task_id)?,
        TaskCommand::Worktree { task_id } => show_worktree(db_path, &task_id)?,
        TaskCommand::Accept { task_id } => {
            OrcApp::open(db_path, ".")?.accept(&task_id)?;
            println!(
                "Accepted task {}; changes integrated and task marked done.",
                task_id
            );
        }
        TaskCommand::Reject { task_id, reason } => {
            OrcApp::open(db_path, ".")?.reject(&task_id, reason.as_deref())?;
            println!(
                "Rejected task {}; worktree preserved and task moved to ready.",
                task_id
            );
        }
        TaskCommand::Depend {
            task_id,
            dependency_id,
        } => match Database::open(db_path) {
            Ok(db) => {
                db.add_task_dependency(&task_id, &dependency_id)
                    .map_err(|e| anyhow::anyhow!(e))?;
                println!("Added dependency: {} depends on {}", task_id, dependency_id);
            }
            Err(_) => eprintln!("No DB found. Run `orc init` to initialize repository state."),
        },
        TaskCommand::Undepend {
            task_id,
            dependency_id,
        } => match Database::open(db_path) {
            Ok(db) => {
                let changed = db
                    .remove_task_dependency(&task_id, &dependency_id)
                    .map_err(|e| anyhow::anyhow!(e))?;
                if !changed {
                    anyhow::bail!("dependency '{}' -> '{}' not found", task_id, dependency_id);
                }
                println!(
                    "Removed dependency: {} no longer depends on {}",
                    task_id, dependency_id
                );
            }
            Err(_) => eprintln!("No DB found. Run `orc init` to initialize repository state."),
        },
    }
    Ok(())
}

fn show_diff(db_path: &str, task_id: &str) -> Result<()> {
    match Database::open(db_path) {
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
    match Database::open(db_path) {
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
