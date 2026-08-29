use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::backend::{self, HealthCommandRunner};
use crate::storage::Database;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckStatus {
    Ok,
    Unavailable(String),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoctorReport {
    pub project: Vec<Check>,
    pub agents: Vec<Check>,
    pub active_tasks: Vec<ActiveTask>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveTask {
    pub task_id: String,
    pub run_status: String,
    pub started_at: String,
}

impl DoctorReport {
    pub fn overall(&self) -> &'static str {
        let mut statuses = self
            .project
            .iter()
            .chain(&self.agents)
            .map(|check| &check.status);
        if statuses
            .clone()
            .any(|status| matches!(status, CheckStatus::Failed(_)))
        {
            "FAILED"
        } else if statuses.any(|status| matches!(status, CheckStatus::Unavailable(_))) {
            "DEGRADED"
        } else {
            "OK"
        }
    }
}

pub struct SystemHealthRunner;

impl HealthCommandRunner for SystemHealthRunner {
    fn executable_exists(&self, executable: &str) -> bool {
        env::var_os("PATH").is_some_and(|paths| {
            env::split_paths(&paths).any(|path| {
                let candidate = path.join(executable);
                candidate.is_file()
            })
        })
    }

    fn run(
        &self,
        executable: &str,
        args: &[&str],
        cwd: &Path,
        environment: Option<(&str, &Path)>,
    ) -> Result<(), String> {
        let mut command = Command::new(executable);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some((key, value)) = environment {
            command.env(key, value);
        }
        let output = command
            .output()
            .map_err(|error| format!("failed to run health check: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            Err(if detail.is_empty() {
                "health command failed".into()
            } else {
                detail
            })
        }
    }
}

pub fn inspect(root: impl AsRef<Path>, runner: &dyn HealthCommandRunner) -> DoctorReport {
    let root = root.as_ref();
    let mut project = Vec::new();
    let git_available = runner.executable_exists("git");
    project.push(check("git", git_available, "Git executable not found"));
    let repository = git_available
        && runner
            .run("git", &["rev-parse", "--is-inside-work-tree"], root, None)
            .is_ok();
    project.push(check(
        "repository",
        repository,
        "not a valid Git repository",
    ));

    let db_path = root.join(".orc/orc.db");
    let database = Database::open(&db_path).and_then(|db| {
        // Doctor audits the authoritative global registry itself, including
        // agents not currently referenced by this project.
        let agents = db.list_agents()?;
        let active_tasks = db
            .list_tasks()?
            .into_iter()
            .filter(|task| task.status == crate::task::TaskStatus::Active)
            .map(|task| {
                let run = db.list_agent_runs_for_task(&task.id).ok().and_then(|runs| {
                    runs.into_iter()
                        .find(|run| matches!(run.status.as_str(), "running" | "waiting_external"))
                });
                ActiveTask {
                    task_id: task.id,
                    run_status: run
                        .as_ref()
                        .map(|run| run.status.clone())
                        .unwrap_or_else(|| "none".into()),
                    started_at: run
                        .map(|run| run.started_at)
                        .unwrap_or_else(|| "none".into()),
                }
            })
            .collect();
        Ok((agents, active_tasks))
    });
    project.push(check(
        "database",
        database.is_ok(),
        "missing or invalid .orc/orc.db",
    ));
    let contract = root.join(".orc/engineering.md");
    project.push(check(
        "engineering contract",
        std::fs::File::open(contract).is_ok(),
        "missing or unreadable .orc/engineering.md",
    ));
    let worktree_parent = root.join(".orc/worktrees");
    let safe_worktree = resolve_safely(root, &worktree_parent);
    project.push(check(
        "worktree path",
        safe_worktree,
        "cannot resolve worktree path safely",
    ));

    let agents = database
        .as_ref()
        .map(|(agents, _active_tasks)| {
            agents
                .iter()
                .filter(|agent| agent.enabled)
                .map(|agent| {
                    let status = match backend::check_health(agent, root, runner) {
                        Ok(()) => CheckStatus::Ok,
                        Err(error) => CheckStatus::Unavailable(error),
                    };
                    Check {
                        name: agent.id.clone(),
                        status,
                        detail: Some(format!(
                            "availability: {}; quota: {}{}",
                            agent.status,
                            agent
                                .quota_remaining_percent
                                .map(|value| format!("{value}%"))
                                .unwrap_or_else(|| "unknown".into()),
                            agent
                                .quota_reset_at
                                .as_deref()
                                .map(|reset| {
                                    format!(", reset: {}", crate::format::timestamp(reset))
                                })
                                .unwrap_or_default()
                        )),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let active_tasks = database
        .map(|(_agents, active_tasks)| active_tasks)
        .unwrap_or_default();
    DoctorReport {
        project,
        agents,
        active_tasks,
    }
}

fn check(name: &str, healthy: bool, failure: &str) -> Check {
    Check {
        name: name.into(),
        status: if healthy {
            CheckStatus::Ok
        } else {
            CheckStatus::Failed(failure.into())
        },
        detail: None,
    }
}

fn resolve_safely(root: &Path, path: &Path) -> bool {
    let canonical_root = match std::fs::canonicalize(root) {
        Ok(path) => path,
        Err(_) => return false,
    };
    let parent = path.parent().unwrap_or(path);
    let resolved_parent =
        std::fs::canonicalize(parent).unwrap_or_else(|_| canonical_root.join(".orc"));
    resolved_parent.starts_with(&canonical_root) && PathBuf::from(path).starts_with(root)
}
