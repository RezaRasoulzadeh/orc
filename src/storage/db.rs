use crate::registry::{AgentDefinition, QuotaLimits, ReasoningEffort};
use crate::task::{Task, TaskPriority, TaskScopeMode, TaskStatus};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};
use std::{io, path::Path};

fn priority_string(priority: TaskPriority) -> &'static str {
    match priority {
        TaskPriority::Low => "low",
        TaskPriority::Normal => "normal",
        TaskPriority::High => "high",
        TaskPriority::Critical => "critical",
    }
}

#[derive(Debug, Clone)]
pub struct AgentRun {
    pub id: i64,
    pub project_id: i64,
    pub task_id: Option<String>,
    pub agent: String,
    pub execution_mode: String,
    pub status: String,
    pub output: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub phase: Option<String>,
    pub last_activity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerResult {
    pub run_id: i64,
    pub outcome: String,
    pub failure_category: Option<String>,
    pub duration_ms: Option<i64>,
    pub metadata: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub id: i64,
    pub reason: String,
    pub resolved: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum DbError {
    #[error("database filesystem error: {0}")]
    Io(#[from] io::Error),
    #[error("sqlite error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("invalid next task id in database: {0}")]
    InvalidSequence(String),
    #[error("quota remaining percent must be between 0 and 100, got {0}")]
    InvalidQuota(i64),
    #[error("invalid or already completed agent run: {0}")]
    InvalidRunStatus(i64),
    #[error("task '{0}' cannot depend on itself")]
    SelfDependency(String),
    #[error("task '{0}' not found")]
    TaskNotFound(String),
    #[error("task '{0}' already depends on '{1}'")]
    DuplicateDependency(String, String),
    #[error("dependency cycle detected: adding '{0}' -> '{1}' would create a cycle")]
    DependencyCycle(String, String),
    #[error("dependency '{0}' -> '{1}' not found")]
    DependencyNotFound(String, String),
    #[error("scheduler error: {0}")]
    Scheduler(String),
    #[error("task '{0}' is not active")]
    TaskNotActive(String),
    #[error("task '{0}' has no non-terminal agent run to recover")]
    NoRecoverableRun(String),
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn project_report(
        &self,
        project_id: i64,
        name: String,
        repository: String,
        engineering_contract: String,
        architecture: crate::protocol::ReportArchitecture,
    ) -> Result<crate::protocol::ProjectReport, DbError> {
        let tasks = self.list_tasks()?;
        let mut counts = std::collections::BTreeMap::new();
        for task in &tasks {
            *counts.entry(task.status.to_string()).or_insert(0) += 1;
        }
        let summaries = tasks
            .iter()
            .map(|task| crate::protocol::TaskSummary {
                id: task.id.clone(),
                title: task.title.clone(),
                status: task.status.to_string(),
            })
            .collect();
        let busy: std::collections::HashSet<_> = self.list_busy_agents()?.into_iter().collect();
        let agents = self
            .list_agents()?
            .into_iter()
            .map(|agent| crate::protocol::ReportAgent {
                id: agent.id.clone(),
                display_name: agent.display_name,
                enabled: agent.enabled,
                status: agent.status,
                execution_mode: agent.execution_mode,
                capabilities: agent.capabilities,
                busy: busy.contains(&agent.id),
            })
            .collect();
        let recent_work = self
            .list_agent_runs(project_id, 20)?
            .into_iter()
            .map(|run| crate::protocol::ReportRun {
                task_id: run.task_id,
                agent: run.agent,
                status: run.status,
                output: run.output,
                finished_at: run.finished_at,
            })
            .collect();
        Ok(crate::protocol::ProjectReport {
            protocol_version: crate::protocol::PROTOCOL_VERSION,
            project: crate::protocol::ReportProject {
                name,
                repository,
                branch: None,
                commit: None,
            },
            engineering_contract,
            architecture,
            lifecycle: crate::protocol::ReportLifecycle {
                counts,
                tasks: summaries,
            },
            agents,
            queue: crate::queue::compute_queue(self)
                .map_err(|e| DbError::Scheduler(e.to_string()))?,
            recent_work,
            risks: Vec::new(),
            open_questions: Vec::new(),
            role_boundaries: vec![
                "Planner proposes a plan; Orc and humans apply or dispatch it.".into(),
            ],
            planning_constraints: vec![
                "Planning is read-only and must not mutate project state or dispatch work.".into(),
            ],
            approval_requirements: vec![
                "A human must review and approve the plan before ApplyPlan.".into(),
            ],
        })
    }

    pub fn planning_project_state(&self) -> Result<crate::protocol::PlanningProjectState, DbError> {
        let tasks = self.list_tasks()?;
        let queue =
            crate::queue::compute_queue(self).map_err(|e| DbError::Scheduler(e.to_string()))?;
        let mut task_counts = std::collections::BTreeMap::new();
        for task in &tasks {
            *task_counts.entry(task.status.to_string()).or_insert(0) += 1;
        }
        let summaries = |status: &str| {
            tasks
                .iter()
                .filter(|task| task.status.to_string() == status)
                .map(|task| crate::protocol::TaskSummary {
                    id: task.id.clone(),
                    title: task.title.clone(),
                    status: status.into(),
                })
                .collect()
        };
        let queue_summaries = |entries: &[crate::queue::QueueEntry], status: &str| {
            entries
                .iter()
                .map(|entry| crate::protocol::TaskSummary {
                    id: entry.task.id.clone(),
                    title: entry.task.title.clone(),
                    status: status.into(),
                })
                .collect()
        };
        let busy_agents = self.list_busy_agents()?;
        let busy: std::collections::HashSet<_> = busy_agents.iter().cloned().collect();
        let usable_agents = self
            .list_agents()?
            .into_iter()
            .filter(|agent| {
                agent.enabled && agent.status == "available" && !busy.contains(&agent.id)
            })
            .map(|agent| agent.id)
            .collect();
        Ok(crate::protocol::PlanningProjectState {
            task_counts,
            ready_tasks: queue_summaries(&queue.ready, "ready"),
            active_tasks: summaries("active"),
            review_tasks: summaries("review"),
            blocked_tasks: queue_summaries(&queue.blocked, "blocked"),
            usable_agents,
            busy_agents,
            quota_reserve_percent: self.quota_reserve()?,
        })
    }

    pub fn project_facts(
        &self,
        project_id: i64,
    ) -> Result<std::collections::BTreeMap<String, String>, DbError> {
        let mut statement = self
            .conn
            .prepare("SELECT key, value FROM project_facts WHERE project_id = ?1 ORDER BY key")?;
        Ok(statement
            .query_map(params![project_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?)
    }

    pub fn apply_plan(
        &self,
        project_id: i64,
        response: &crate::protocol::PlanResponse,
    ) -> Result<std::collections::BTreeMap<String, String>, DbError> {
        response
            .validate()
            .map_err(|e| DbError::Scheduler(e.to_string()))?;
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            let mut mapping = std::collections::BTreeMap::new();
            for task in &response.tasks {
                let id = self.allocate_task_id()?;
                let priority = priority_string(task.priority);
                self.conn.execute("INSERT INTO tasks (id, project_id, title, objective, role, priority, status, required_capabilities, scope_mode, context_files, expected_changes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'backlog', ?7, ?8, ?9, ?10)", params![id, project_id, task.title, task.objective, task.role, priority, serde_json::to_string(&task.capabilities)?, task.scope_mode.map(|v| v.to_string()), serde_json::to_string(&task.context_files)?, serde_json::to_string(&task.expected_changes)?])?;
                mapping.insert(task.local_id.clone(), id);
            }
            for task in &response.tasks {
                for dependency in &task.depends_on {
                    self.add_task_dependency(
                        mapping[&task.local_id].as_str(),
                        mapping[dependency].as_str(),
                    )?;
                }
            }
            Ok::<_, DbError>(mapping)
        })();
        match result {
            Ok(mapping) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(mapping)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }
    pub fn init(path: impl AsRef<Path>) -> Result<Self, DbError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path.as_ref())?;
        Self::configure(&conn)?;
        conn.execute_batch(
            r#"
            BEGIN;
            CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS projects (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            );
            CREATE TABLE IF NOT EXISTS project_facts (
                project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                PRIMARY KEY (project_id, key)
            );
            CREATE TABLE IF NOT EXISTS agents (
                id TEXT PRIMARY KEY,
                backend TEXT NOT NULL,
                display_name TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                priority INTEGER NOT NULL DEFAULT 0,
                capabilities TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'available',
                unavailable_reason TEXT,
                profile_path TEXT,
                model TEXT,
                reasoning_effort TEXT,
                config_metadata TEXT,
                execution_mode TEXT NOT NULL DEFAULT 'automated',
                quota_remaining_percent INTEGER,
                quota_reset_at TEXT,
                quota_checked_at TEXT,
                quota_source TEXT
                , quota_limits TEXT
            );
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id),
                title TEXT NOT NULL,
                objective TEXT NOT NULL,
                role TEXT NOT NULL,
                priority TEXT NOT NULL,
                status TEXT NOT NULL,
                required_capabilities TEXT,
                scope_mode TEXT,
                context_files TEXT,
                expected_changes TEXT,
                cancellation_reason TEXT,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP),
                updated_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            );
            CREATE TABLE IF NOT EXISTS task_dependencies (
                task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                depends_on TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                PRIMARY KEY (task_id, depends_on)
            );
            CREATE TABLE IF NOT EXISTS decisions (
                id INTEGER PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id),
                task_id TEXT REFERENCES tasks(id),
                summary TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            );
            CREATE TABLE IF NOT EXISTS approval_requests (
                id INTEGER PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id),
                reason TEXT NOT NULL,
                resolved INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            );
            CREATE TABLE IF NOT EXISTS agent_runs (
                id INTEGER PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id),
                task_id TEXT REFERENCES tasks(id),
                agent TEXT NOT NULL,
                execution_mode TEXT NOT NULL DEFAULT 'automated',
                status TEXT NOT NULL,
                output TEXT,
                started_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP),
                finished_at TEXT
                , phase TEXT
                , last_activity TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            );
            CREATE TABLE IF NOT EXISTS worker_results (
                run_id INTEGER PRIMARY KEY REFERENCES agent_runs(id) ON DELETE CASCADE,
                outcome TEXT NOT NULL,
                failure_category TEXT,
                duration_ms INTEGER,
                metadata TEXT
            );
            CREATE TABLE IF NOT EXISTS worktree_metadata (
                agent_run_id INTEGER PRIMARY KEY REFERENCES agent_runs(id) ON DELETE CASCADE,
                task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                branch_name TEXT NOT NULL,
                worktree_path TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            );
            INSERT OR IGNORE INTO meta (key, value) VALUES ('next_task_id', '1');
            COMMIT;
            "#,
        )?;
        Self::ensure_agent_columns(&conn)?;
        Self::ensure_agent_run_columns(&conn)?;
        Self::ensure_worker_results_table(&conn)?;
        Self::ensure_task_columns(&conn)?;
        Self::ensure_approval_request_columns(&conn)?;
        Ok(Self { conn })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, DbError> {
        if !path.as_ref().exists() {
            return Err(DbError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("database does not exist: {}", path.as_ref().display()),
            )));
        }
        let conn = Connection::open_with_flags(path.as_ref(), OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        Self::configure(&conn)?;
        Self::ensure_registry_schema(&conn)?;
        Ok(Self { conn })
    }

    fn configure(conn: &Connection) -> Result<(), DbError> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Ok(())
    }

    fn ensure_registry_schema(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS project_facts (project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE, key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY (project_id, key)); CREATE TABLE IF NOT EXISTS agents (id TEXT PRIMARY KEY, backend TEXT NOT NULL, display_name TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1, priority INTEGER NOT NULL DEFAULT 0, capabilities TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'available', unavailable_reason TEXT, profile_path TEXT, config_metadata TEXT);",
        )?;
        Self::ensure_agent_columns(conn)?;
        Self::ensure_agent_run_columns(conn)?;
        Self::ensure_worker_results_table(conn)?;
        Self::ensure_task_columns(conn)?;
        Self::ensure_approval_request_columns(conn)?;
        Ok(())
    }

    fn ensure_approval_request_columns(conn: &Connection) -> Result<(), DbError> {
        let mut statement = conn.prepare("PRAGMA table_info(approval_requests)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|column| column == "resolved") {
            conn.execute_batch(
                "ALTER TABLE approval_requests ADD COLUMN resolved INTEGER NOT NULL DEFAULT 0",
            )?;
        }
        Ok(())
    }

    fn ensure_task_columns(conn: &Connection) -> Result<(), DbError> {
        let mut statement = conn.prepare("PRAGMA table_info(tasks)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns
            .iter()
            .any(|column| column == "required_capabilities")
        {
            conn.execute_batch("ALTER TABLE tasks ADD COLUMN required_capabilities TEXT")?;
        }
        for (name, definition) in [
            ("scope_mode", "TEXT"),
            ("context_files", "TEXT"),
            ("expected_changes", "TEXT"),
            ("cancellation_reason", "TEXT"),
        ] {
            if !columns.iter().any(|column| column == name) {
                conn.execute_batch(&format!("ALTER TABLE tasks ADD COLUMN {name} {definition}"))?;
            }
        }
        Ok(())
    }

    fn ensure_agent_columns(conn: &Connection) -> Result<(), DbError> {
        let mut statement = conn.prepare("PRAGMA table_info(agents)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        for (name, definition) in [
            ("quota_remaining_percent", "INTEGER"),
            ("quota_reset_at", "TEXT"),
            ("quota_checked_at", "TEXT"),
            ("quota_source", "TEXT"),
            ("quota_limits", "TEXT"),
            ("execution_mode", "TEXT NOT NULL DEFAULT 'automated'"),
            ("model", "TEXT"),
            ("reasoning_effort", "TEXT"),
        ] {
            if !columns.iter().any(|column| column == name) {
                conn.execute_batch(&format!(
                    "ALTER TABLE agents ADD COLUMN {name} {definition}"
                ))?;
            }
        }
        Ok(())
    }

    fn ensure_agent_run_columns(conn: &Connection) -> Result<(), DbError> {
        let mut statement = conn.prepare("PRAGMA table_info(agent_runs)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|column| column == "execution_mode") {
            conn.execute_batch(
                "ALTER TABLE agent_runs ADD COLUMN execution_mode TEXT NOT NULL DEFAULT 'automated'",
            )?;
        }
        for (name, definition) in [("phase", "TEXT"), ("last_activity", "TEXT")] {
            if !columns.iter().any(|column| column == name) {
                conn.execute_batch(&format!(
                    "ALTER TABLE agent_runs ADD COLUMN {name} {definition}"
                ))?;
            }
        }

        conn.execute(
            "UPDATE agent_runs
             SET last_activity = COALESCE(finished_at, started_at, CURRENT_TIMESTAMP)
             WHERE last_activity IS NULL",
            [],
        )?;

        Ok(())
    }

    fn ensure_worker_results_table(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS worker_results (run_id INTEGER PRIMARY KEY REFERENCES agent_runs(id) ON DELETE CASCADE, outcome TEXT NOT NULL, failure_category TEXT, duration_ms INTEGER, metadata TEXT)")?;
        Ok(())
    }

    pub fn create_project(&self, name: &str) -> Result<i64, DbError> {
        self.conn
            .execute("INSERT INTO projects (name) VALUES (?1)", params![name])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_project_name(&self) -> Result<Option<String>, DbError> {
        Ok(self
            .conn
            .query_row("SELECT name FROM projects ORDER BY id LIMIT 1", [], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn get_project_id(&self) -> Result<Option<i64>, DbError> {
        Ok(self
            .conn
            .query_row("SELECT id FROM projects ORDER BY id LIMIT 1", [], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn insert_agent(&self, agent: &AgentDefinition) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO agents (id, backend, display_name, enabled, priority, capabilities, status, unavailable_reason, profile_path, model, reasoning_effort, config_metadata, execution_mode, quota_remaining_percent, quota_reset_at, quota_checked_at, quota_source, quota_limits) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                agent.id,
                agent.backend,
                agent.display_name,
                agent.enabled,
                agent.priority,
                serde_json::to_string(&agent.capabilities)?,
                agent.status,
                agent.unavailable_reason,
                agent.profile_path,
                agent.model,
                agent.reasoning_effort.map(ReasoningEffort::as_str),
                agent.config_metadata,
                agent.execution_mode,
                agent.quota_remaining_percent,
                agent.quota_reset_at,
                agent.quota_checked_at,
                agent.quota_source,
                agent.quota_limits.as_ref().map(serde_json::to_string).transpose()?,
            ],
        )?;
        Ok(())
    }

    fn agent_from_row(row: &Row<'_>) -> rusqlite::Result<AgentDefinition> {
        let capabilities_json: String = row.get(5)?;
        let capabilities = serde_json::from_str(&capabilities_json).map_err(|error| {
            rusqlite::Error::InvalidParameterName(format!("invalid agent capabilities: {error}"))
        })?;
        let quota_limits_json: Option<String> = row.get(17)?;
        let quota_limits = quota_limits_json
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| {
                rusqlite::Error::InvalidParameterName(format!("invalid quota limits: {error}"))
            })?;
        Ok(AgentDefinition {
            id: row.get(0)?,
            backend: row.get(1)?,
            execution_mode: row.get(12)?,
            display_name: row.get(2)?,
            enabled: row.get::<_, i64>(3)? != 0,
            priority: row.get(4)?,
            capabilities,
            status: row.get(6)?,
            unavailable_reason: row.get(7)?,
            profile_path: row.get(8)?,
            model: row.get(9)?,
            reasoning_effort: row
                .get::<_, Option<String>>(10)?
                .map(|value| ReasoningEffort::parse(&value))
                .transpose()
                .map_err(|error| rusqlite::Error::InvalidParameterName(error.to_string()))?,
            config_metadata: row.get(11)?,
            quota_remaining_percent: row.get(13)?,
            quota_reset_at: row.get(14)?,
            quota_checked_at: row.get(15)?,
            quota_source: row.get(16)?,
            quota_limits,
        })
    }

    pub fn get_agent(&self, id: &str) -> Result<Option<AgentDefinition>, DbError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, backend, display_name, enabled, priority, capabilities, status, unavailable_reason, profile_path, model, reasoning_effort, config_metadata, execution_mode, quota_remaining_percent, quota_reset_at, quota_checked_at, quota_source, quota_limits FROM agents WHERE id = ?1",
                params![id],
                Self::agent_from_row,
            )
            .optional()?)
    }

    pub fn list_agents(&self) -> Result<Vec<AgentDefinition>, DbError> {
        let mut statement = self.conn.prepare(
            "SELECT id, backend, display_name, enabled, priority, capabilities, status, unavailable_reason, profile_path, model, reasoning_effort, config_metadata, execution_mode, quota_remaining_percent, quota_reset_at, quota_checked_at, quota_source, quota_limits FROM agents ORDER BY id",
        )?;
        Ok(statement
            .query_map([], Self::agent_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn set_agent_enabled(&self, id: &str, enabled: bool) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "UPDATE agents SET enabled = ?1 WHERE id = ?2",
            params![enabled, id],
        )? != 0)
    }

    pub fn set_agent_priority(&self, id: &str, priority: i64) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "UPDATE agents SET priority = ?1 WHERE id = ?2",
            params![priority, id],
        )? != 0)
    }

    pub fn set_agent_profile_path(&self, id: &str, profile_path: &str) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "UPDATE agents SET profile_path = ?1 WHERE id = ?2",
            params![profile_path, id],
        )? != 0)
    }

    pub fn set_agent_model(&self, id: &str, model: &str) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "UPDATE agents SET model = ?1 WHERE id = ?2",
            params![model, id],
        )? != 0)
    }

    pub fn set_agent_reasoning_effort(
        &self,
        id: &str,
        effort: ReasoningEffort,
    ) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "UPDATE agents SET reasoning_effort = ?1 WHERE id = ?2",
            params![effort.as_str(), id],
        )? != 0)
    }

    pub fn set_agent_execution_mode(
        &self,
        id: &str,
        execution_mode: &str,
    ) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "UPDATE agents SET execution_mode = ?1 WHERE id = ?2",
            params![execution_mode, id],
        )? != 0)
    }

    pub fn set_agent_quota(
        &self,
        id: &str,
        remaining_percent: i64,
        reset_at: Option<&str>,
    ) -> Result<bool, DbError> {
        if !(0..=100).contains(&remaining_percent) {
            return Err(DbError::InvalidQuota(remaining_percent));
        }
        Ok(self.conn.execute(
            "UPDATE agents SET quota_remaining_percent = ?1, quota_reset_at = ?2, quota_checked_at = CURRENT_TIMESTAMP, quota_source = 'manual', quota_limits = NULL WHERE id = ?3",
            params![remaining_percent, reset_at, id],
        )? != 0)
    }

    pub fn quota_reserve(&self) -> Result<i64, DbError> {
        let value: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'quota_reserve'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        value
            .map(|value| {
                value
                    .parse::<i64>()
                    .map_err(|_| rusqlite::Error::InvalidQuery)
            })
            .transpose()
            .map(|value| value.unwrap_or(0))
            .map_err(DbError::from)
    }

    pub fn set_quota_reserve(&self, reserve: i64) -> Result<(), DbError> {
        if !(0..=100).contains(&reserve) {
            return Err(DbError::InvalidQuota(reserve));
        }
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES ('quota_reserve', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![reserve.to_string()],
        )?;
        Ok(())
    }

    pub fn set_agent_synced_quota(
        &self,
        id: &str,
        remaining_percent: i64,
        reset_at: Option<&str>,
        source: &str,
        limits: &QuotaLimits,
    ) -> Result<bool, DbError> {
        if !(0..=100).contains(&remaining_percent) {
            return Err(DbError::InvalidQuota(remaining_percent));
        }
        Ok(self.conn.execute(
            "UPDATE agents SET quota_remaining_percent = ?1, quota_reset_at = ?2, quota_checked_at = CURRENT_TIMESTAMP, quota_source = ?3, quota_limits = ?4 WHERE id = ?5",
            params![remaining_percent, reset_at, source, serde_json::to_string(limits)?, id],
        )? != 0)
    }

    pub fn clear_agent_quota(&self, id: &str) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "UPDATE agents SET quota_remaining_percent = NULL, quota_reset_at = NULL, quota_checked_at = NULL, quota_source = NULL, quota_limits = NULL WHERE id = ?1",
            params![id],
        )? != 0)
    }

    pub fn set_agent_availability(
        &self,
        id: &str,
        status: &str,
        reason: Option<&str>,
    ) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "UPDATE agents SET status = ?1, unavailable_reason = ?2 WHERE id = ?3",
            params![status, reason, id],
        )? != 0)
    }

    pub fn store_discovery_facts(
        &self,
        project_id: i64,
        response: &crate::protocol::ProjectDiscoveryResponse,
    ) -> Result<(), DbError> {
        let facts = [
            ("purpose", response.project.purpose.clone()),
            (
                "languages",
                serde_json::to_string(&response.project.languages)?,
            ),
            (
                "build_commands",
                serde_json::to_string(&response.engineering.build_commands)?,
            ),
            (
                "test_commands",
                serde_json::to_string(&response.engineering.test_commands)?,
            ),
            (
                "modules",
                serde_json::to_string(&response.architecture.modules)?,
            ),
            (
                "boundaries",
                serde_json::to_string(&response.architecture.boundaries)?,
            ),
            (
                "entry_points",
                serde_json::to_string(&response.architecture.entry_points)?,
            ),
            (
                "observed_patterns",
                serde_json::to_string(&response.engineering.observed_patterns)?,
            ),
        ];
        for (key, value) in facts {
            self.conn.execute(
                "INSERT INTO project_facts (project_id, key, value) VALUES (?1, ?2, ?3) ON CONFLICT(project_id, key) DO UPDATE SET value = excluded.value",
                params![project_id, key, value],
            )?;
        }
        Ok(())
    }

    fn task_from_row(row: &Row<'_>) -> rusqlite::Result<Task> {
        let priority: String = row.get(4)?;
        let status: String = row.get(5)?;
        let priority_value = match priority.as_str() {
            "low" => TaskPriority::Low,
            "normal" => TaskPriority::Normal,
            "high" => TaskPriority::High,
            "critical" => TaskPriority::Critical,
            _ => {
                return Err(rusqlite::Error::InvalidParameterName(format!(
                    "invalid priority: {priority}"
                )));
            }
        };
        let status_value = match status.as_str() {
            "backlog" => TaskStatus::Backlog,
            "ready" => TaskStatus::Ready,
            "active" => TaskStatus::Active,
            "review" => TaskStatus::Review,
            "blocked" => TaskStatus::Blocked,
            "done" => TaskStatus::Done,
            "cancelled" => TaskStatus::Cancelled,
            _ => {
                return Err(rusqlite::Error::InvalidParameterName(format!(
                    "invalid status: {status}"
                )));
            }
        };
        let required_capabilities_json: Option<String> = row.get(6)?;
        let required_capabilities = match required_capabilities_json {
            Some(json_str) if !json_str.trim().is_empty() => serde_json::from_str(&json_str)
                .map_err(|error| {
                    rusqlite::Error::InvalidParameterName(format!(
                        "invalid task capabilities: {error}"
                    ))
                })?,
            _ => Vec::new(),
        };
        let scope_mode = match row.get::<_, Option<String>>(7)? {
            Some(value) => Some(TaskScopeMode::parse(&value).ok_or_else(|| {
                rusqlite::Error::InvalidParameterName(format!("invalid task scope mode: {value}"))
            })?),
            None => None,
        };
        let list = |index| -> Result<Vec<String>, rusqlite::Error> {
            match row.get::<_, Option<String>>(index)? {
                Some(value) => serde_json::from_str(&value).map_err(|error| {
                    rusqlite::Error::InvalidParameterName(format!("invalid task metadata: {error}"))
                }),
                None => Ok(Vec::new()),
            }
        };
        Ok(Task {
            id: row.get(0)?,
            title: row.get(1)?,
            objective: row.get(2)?,
            role: row.get(3)?,
            priority: priority_value,
            status: status_value,
            cancellation_reason: row.get(10)?,
            required_capabilities,
            scope_mode,
            context_files: list(8)?,
            expected_changes: list(9)?,
        })
    }

    pub fn list_tasks(&self) -> Result<Vec<Task>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, objective, role, priority, status, required_capabilities, scope_mode, context_files, expected_changes, cancellation_reason FROM tasks ORDER BY created_at",
        )?;
        Ok(stmt
            .query_map([], Self::task_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    #[allow(dead_code)]
    pub fn insert_task(
        &self,
        project_id: i64,
        title: &str,
        objective: &str,
        role: &str,
        priority: TaskPriority,
    ) -> Result<String, DbError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            let id = self.allocate_task_id()?;
            let priority_str = priority_string(priority);
            self.conn.execute("INSERT INTO tasks (id, project_id, title, objective, role, priority, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'backlog')", params![id, project_id, title, objective, role, priority_str])?;
            Ok(id)
        })();
        match result {
            Ok(id) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(id)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    fn allocate_task_id(&self) -> Result<String, DbError> {
        let value: String = self.conn.query_row(
            "SELECT value FROM meta WHERE key = 'next_task_id'",
            [],
            |r| r.get(0),
        )?;
        let seq = value
            .parse::<u64>()
            .map_err(|_| DbError::InvalidSequence(value.clone()))?;
        self.conn.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'next_task_id'",
            params![(seq + 1).to_string()],
        )?;
        Ok(format!("T-{seq:04}"))
    }

    /// Insert a task with a specific ID (mainly for testing).
    #[allow(dead_code)]
    pub fn insert_task_with_id(
        &self,
        project_id: i64,
        id: &str,
        title: &str,
        objective: &str,
        role: &str,
        priority: TaskPriority,
    ) -> Result<String, DbError> {
        let priority_str = match priority {
            TaskPriority::Low => "low",
            TaskPriority::Normal => "normal",
            TaskPriority::High => "high",
            TaskPriority::Critical => "critical",
        };
        self.conn.execute(
            "INSERT INTO tasks (id, project_id, title, objective, role, priority, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'backlog')",
            params![id, project_id, title, objective, role, priority_str],
        )?;
        Ok(id.to_string())
    }

    /// Apply an Engineering Lead response atomically.
    /// All actions from the response are applied inside a single SQLite transaction.
    /// If any action fails, the transaction is rolled back and no changes from this
    /// response are persisted.
    pub fn apply_engineering_lead_response(
        &self,
        project_id: i64,
        response: &crate::protocol::EngineeringLeadResponse,
    ) -> Result<(), DbError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            for action in &response.actions {
                match action {
                    crate::protocol::LeadAction::CreateTask {
                        title,
                        objective,
                        role,
                        priority,
                        scope_mode,
                        context_files,
                        expected_changes,
                    } => {
                        // get seq
                        let value: String = self.conn.query_row(
                            "SELECT value FROM meta WHERE key = 'next_task_id'",
                            [],
                            |r| r.get(0),
                        )?;
                        let seq = value
                            .parse::<u64>()
                            .map_err(|_| DbError::InvalidSequence(value.clone()))?;
                        let id = format!("T-{seq:04}");
                        let priority_str = match priority {
                            TaskPriority::Low => "low",
                            TaskPriority::Normal => "normal",
                            TaskPriority::High => "high",
                            TaskPriority::Critical => "critical",
                        };
                        self.conn.execute(
                            "INSERT INTO tasks (id, project_id, title, objective, role, priority, status, scope_mode, context_files, expected_changes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'backlog', ?7, ?8, ?9)",
                            params![id, project_id, title, objective, role, priority_str, scope_mode.map(|v| v.to_string()), serde_json::to_string(context_files)?, serde_json::to_string(expected_changes)?],
                        )?;
                        self.conn.execute(
                            "UPDATE meta SET value = ?1 WHERE key = 'next_task_id'",
                            params![(seq + 1).to_string()],
                        )?;
                    }
                    crate::protocol::LeadAction::RequireCtoApproval { reason } => {
                        self.conn.execute(
                            "INSERT INTO approval_requests (project_id, reason) VALUES (?1, ?2)",
                            params![project_id, reason],
                        )?;
                    }
                }
            }
            Ok(())
        })();

        match result {
            Ok(_) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(err) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(err)
            }
        }
    }

    pub fn get_task(&self, id: &str) -> Result<Option<Task>, DbError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, title, objective, role, priority, status, required_capabilities, scope_mode, context_files, expected_changes, cancellation_reason FROM tasks WHERE id = ?1",
                params![id],
                Self::task_from_row,
            )
            .optional()?)
    }

    pub fn set_task_required_capabilities(
        &self,
        id: &str,
        capabilities: &[String],
    ) -> Result<bool, DbError> {
        let json = serde_json::to_string(capabilities)?;
        let changed = self.conn.execute(
            "UPDATE tasks SET required_capabilities = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![json, id],
        )?;
        Ok(changed != 0)
    }

    pub fn set_task_scope(&self, id: &str, scope: TaskScopeMode) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "UPDATE tasks SET scope_mode = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![scope.to_string(), id],
        )? != 0)
    }

    pub fn set_task_context(&self, id: &str, files: &[String]) -> Result<bool, DbError> {
        self.set_task_metadata(id, "context_files", files)
    }
    pub fn set_task_expected_changes(&self, id: &str, files: &[String]) -> Result<bool, DbError> {
        self.set_task_metadata(id, "expected_changes", files)
    }
    fn set_task_metadata(&self, id: &str, column: &str, files: &[String]) -> Result<bool, DbError> {
        let json = serde_json::to_string(files)?;
        Ok(self.conn.execute(
            &format!(
                "UPDATE tasks SET {column} = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2"
            ),
            params![json, id],
        )? != 0)
    }

    #[allow(dead_code)]
    pub fn update_task_status(&self, id: &str, status: TaskStatus) -> Result<bool, DbError> {
        let changed = self.conn.execute(
            "UPDATE tasks SET status = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![status.to_string(), id],
        )?;
        Ok(changed != 0)
    }

    pub fn requeue_task(&self, id: &str, reason: &str) -> Result<(), DbError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            let status: Option<String> = self
                .conn
                .query_row(
                    "SELECT status FROM tasks WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()?;
            match status.as_deref() {
                Some("active") => {}
                Some(_) => return Err(DbError::TaskNotActive(id.into())),
                None => return Err(DbError::TaskNotFound(id.into())),
            }
            let run_id: Option<i64> = self.conn.query_row(
                "SELECT id FROM agent_runs WHERE task_id = ?1 AND status IN ('running', 'waiting_external') ORDER BY started_at DESC, id DESC LIMIT 1",
                params![id], |row| row.get(0),
            ).optional()?;
            let run_id = run_id.ok_or_else(|| DbError::NoRecoverableRun(id.into()))?;
            self.conn.execute(
                "UPDATE agent_runs SET status = 'failed', output = ?1, finished_at = CURRENT_TIMESTAMP WHERE id = ?2 AND status IN ('running', 'waiting_external')",
                params![reason, run_id],
            )?;
            self.conn.execute(
                "UPDATE tasks SET status = 'backlog', updated_at = CURRENT_TIMESTAMP WHERE id = ?1 AND status = 'active'",
                params![id],
            )?;
            Ok::<_, DbError>(())
        })();
        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub fn cancel_task(&self, id: &str, reason: Option<&str>) -> Result<bool, DbError> {
        let changed = self.conn.execute(
            "UPDATE tasks SET status = 'cancelled', cancellation_reason = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2 AND status != 'done' AND status != 'cancelled'",
            params![reason, id],
        )?;
        Ok(changed != 0)
    }

    #[allow(dead_code)]
    pub fn insert_decision(
        &self,
        project_id: i64,
        task_id: Option<&str>,
        summary: &str,
    ) -> Result<i64, DbError> {
        self.conn.execute(
            "INSERT INTO decisions (project_id, task_id, summary) VALUES (?1, ?2, ?3)",
            params![project_id, task_id, summary],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    #[allow(dead_code)]
    pub fn insert_approval_request(&self, project_id: i64, reason: &str) -> Result<i64, DbError> {
        self.conn.execute(
            "INSERT INTO approval_requests (project_id, reason) VALUES (?1, ?2)",
            params![project_id, reason],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    #[allow(dead_code)]
    pub fn list_approval_requests(&self, project_id: i64) -> Result<Vec<ApprovalRequest>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, reason, resolved FROM approval_requests WHERE project_id = ?1 ORDER BY id",
        )?;
        Ok(stmt
            .query_map(params![project_id], |r| {
                Ok(ApprovalRequest {
                    id: r.get(0)?,
                    reason: r.get(1)?,
                    resolved: r.get::<_, i64>(2)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn resolve_approval_request(&self, project_id: i64, id: i64) -> Result<bool, DbError> {
        let changed = self.conn.execute(
            "UPDATE approval_requests SET resolved = 1 WHERE id = ?1 AND project_id = ?2",
            params![id, project_id],
        )?;
        Ok(changed != 0)
    }

    fn agent_run_from_row(row: &Row<'_>) -> rusqlite::Result<AgentRun> {
        Ok(AgentRun {
            id: row.get(0)?,
            project_id: row.get(1)?,
            task_id: row.get(2)?,
            agent: row.get(3)?,
            execution_mode: row.get(4)?,
            status: row.get(5)?,
            output: row.get(6)?,
            started_at: row.get(7)?,
            finished_at: row.get(8)?,
            phase: row.get(9)?,
            last_activity: row.get(10)?,
        })
    }

    pub fn create_agent_run(
        &self,
        project_id: i64,
        task_id: &str,
        agent: &str,
    ) -> Result<i64, DbError> {
        self.create_agent_run_with_mode(project_id, task_id, agent, "automated")
    }

    pub fn create_agent_run_with_mode(
        &self,
        project_id: i64,
        task_id: &str,
        agent: &str,
        execution_mode: &str,
    ) -> Result<i64, DbError> {
        self.conn.execute(
            "INSERT INTO agent_runs (project_id, task_id, agent, execution_mode, status, started_at, phase, last_activity) VALUES (?1, ?2, ?3, ?4, 'running', CURRENT_TIMESTAMP, 'starting', CURRENT_TIMESTAMP)",
            params![project_id, task_id, agent, execution_mode],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn update_agent_run_status(
        &self,
        run_id: i64,
        status: &str,
        output: Option<&str>,
    ) -> Result<bool, DbError> {
        let changed = self.conn.execute(
            "UPDATE agent_runs SET status = ?1, output = ?2, finished_at = CURRENT_TIMESTAMP, last_activity = CURRENT_TIMESTAMP WHERE id = ?3",
            params![status, output, run_id],
        )?;
        if changed != 0 && matches!(status, "completed" | "failed" | "no_changes") {
            self.record_worker_result(run_id, status, output)?;
        }
        Ok(changed != 0)
    }

    fn record_worker_result(
        &self,
        run_id: i64,
        status: &str,
        output: Option<&str>,
    ) -> Result<(), DbError> {
        let outcome = match status {
            "completed" => "success",
            "no_changes" => "no_changes",
            _ if output.is_some_and(|value| value.to_ascii_lowercase().contains("timed out")) => {
                "timeout"
            }
            _ if output.is_some_and(|value| value.contains("Validation")) => "validation_failure",
            _ => "worker_failure",
        };
        let failure_category = match outcome {
            "success" | "no_changes" => None,
            value => Some(value),
        };
        self.conn.execute(
            "INSERT OR REPLACE INTO worker_results (run_id, outcome, failure_category, duration_ms, metadata) SELECT ?1, ?2, ?3, (unixepoch(finished_at) - unixepoch(started_at)) * 1000, ?4 FROM agent_runs WHERE id = ?1",
            params![run_id, outcome, failure_category, format!("{{\"run_status\":\"{status}\"}}")],
        )?;
        Ok(())
    }

    pub fn insert_worker_result(&self, result: &WorkerResult) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO worker_results (run_id, outcome, failure_category, duration_ms, metadata) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![result.run_id, result.outcome, result.failure_category, result.duration_ms, result.metadata],
        )?;
        Ok(())
    }

    pub fn get_worker_result(&self, run_id: i64) -> Result<Option<WorkerResult>, DbError> {
        Ok(self.conn.query_row(
            "SELECT run_id, outcome, failure_category, duration_ms, metadata FROM worker_results WHERE run_id = ?1",
            params![run_id],
            |row| Ok(WorkerResult { run_id: row.get(0)?, outcome: row.get(1)?, failure_category: row.get(2)?, duration_ms: row.get(3)?, metadata: row.get(4)? }),
        ).optional()?)
    }

    pub fn set_agent_run_waiting_external(&self, run_id: i64) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "UPDATE agent_runs SET status = 'waiting_external' WHERE id = ?1 AND status = 'running'",
            params![run_id],
        )? != 0)
    }

    pub fn get_agent_run(&self, run_id: i64) -> Result<Option<AgentRun>, DbError> {
        Ok(self.conn.query_row(
            "SELECT id, project_id, task_id, agent, execution_mode, status, output, started_at, finished_at, phase, last_activity FROM agent_runs WHERE id = ?1",
            params![run_id], Self::agent_run_from_row).optional()?)
    }

    pub fn complete_manual_run(&self, run_id: i64, output: &str) -> Result<String, DbError> {
        let task_id: String = self.conn.query_row(
            "SELECT task_id FROM agent_runs WHERE id = ?1 AND status = 'waiting_external'",
            params![run_id],
            |row| row.get(0),
        )?;
        let changed = self.conn.execute(
            "UPDATE agent_runs SET status = 'completed', output = ?1, finished_at = CURRENT_TIMESTAMP WHERE id = ?2 AND status = 'waiting_external'",
            params![output, run_id])?;
        if changed == 0 {
            return Err(DbError::InvalidRunStatus(run_id));
        }
        self.record_worker_result(run_id, "completed", Some(output))?;
        Ok(task_id)
    }

    pub fn fail_run(&self, run_id: i64, reason: &str) -> Result<String, DbError> {
        let task_id: String = self.conn.query_row(
            "SELECT task_id FROM agent_runs WHERE id = ?1 AND status IN ('running', 'waiting_external')",
            params![run_id], |row| row.get(0))?;
        let changed = self.conn.execute(
            "UPDATE agent_runs SET status = 'failed', output = ?1, finished_at = CURRENT_TIMESTAMP WHERE id = ?2 AND status IN ('running', 'waiting_external')",
            params![reason, run_id])?;
        if changed == 0 {
            return Err(DbError::InvalidRunStatus(run_id));
        }
        self.record_worker_result(run_id, "failed", Some(reason))?;
        Ok(task_id)
    }

    pub fn list_agent_runs(&self, project_id: i64, limit: usize) -> Result<Vec<AgentRun>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, task_id, agent, execution_mode, status, output, started_at, finished_at, phase, last_activity FROM agent_runs WHERE project_id = ?1 ORDER BY started_at DESC LIMIT ?2",
        )?;
        Ok(stmt
            .query_map(params![project_id, limit as i64], Self::agent_run_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn list_agent_runs_for_task(&self, task_id: &str) -> Result<Vec<AgentRun>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, task_id, agent, execution_mode, status, output, started_at, finished_at, phase, last_activity FROM agent_runs WHERE task_id = ?1 ORDER BY started_at DESC, id DESC",
        )?;
        Ok(stmt
            .query_map(params![task_id], Self::agent_run_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn list_busy_agents(&self) -> Result<Vec<String>, DbError> {
        let mut stmt = self.conn.prepare("SELECT DISTINCT agent FROM agent_runs WHERE status IN ('running', 'waiting_external') ORDER BY agent")?;
        Ok(stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn update_agent_run_output(&self, run_id: i64, output: &str) -> Result<bool, DbError> {
        let changed = self.conn.execute(
            "UPDATE agent_runs SET output = ?1 WHERE id = ?2",
            params![output, run_id],
        )?;
        Ok(changed != 0)
    }

    pub fn update_agent_run_phase(&self, run_id: i64, phase: &str) -> Result<bool, DbError> {
        Ok(self.conn.execute("UPDATE agent_runs SET phase = ?1, last_activity = CURRENT_TIMESTAMP WHERE id = ?2 AND status IN ('running', 'waiting_external')", params![phase, run_id])? != 0)
    }

    pub fn store_worktree_metadata(
        &self,
        agent_run_id: i64,
        task_id: &str,
        branch_name: &str,
        worktree_path: &str,
    ) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO worktree_metadata (agent_run_id, task_id, branch_name, worktree_path) VALUES (?1, ?2, ?3, ?4)",
            params![agent_run_id, task_id, branch_name, worktree_path],
        )?;
        Ok(())
    }

    pub fn get_worktree_metadata(
        &self,
        task_id: &str,
    ) -> Result<Option<(String, String)>, DbError> {
        Ok(self
            .conn
            .query_row(
                "SELECT branch_name, worktree_path FROM worktree_metadata WHERE task_id = ?1 ORDER BY created_at DESC LIMIT 1",
                params![task_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    pub fn add_task_dependency(&self, task_id: &str, depends_on: &str) -> Result<(), DbError> {
        if task_id == depends_on {
            return Err(DbError::SelfDependency(task_id.to_string()));
        }

        let task_exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM tasks WHERE id = ?1",
                params![task_id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !task_exists {
            return Err(DbError::TaskNotFound(task_id.to_string()));
        }

        let depends_on_exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM tasks WHERE id = ?1",
                params![depends_on],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !depends_on_exists {
            return Err(DbError::TaskNotFound(depends_on.to_string()));
        }

        let already_exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM task_dependencies WHERE task_id = ?1 AND depends_on = ?2",
                params![task_id, depends_on],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if already_exists {
            return Err(DbError::DuplicateDependency(
                task_id.to_string(),
                depends_on.to_string(),
            ));
        }

        // Cycle check: can `depends_on` reach `task_id` via existing dependencies?
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(depends_on.to_string());
        visited.insert(depends_on.to_string());

        while let Some(current) = queue.pop_front() {
            let deps = self.list_task_dependencies(&current)?;
            for dep in deps {
                if dep == task_id {
                    return Err(DbError::DependencyCycle(
                        task_id.to_string(),
                        depends_on.to_string(),
                    ));
                }
                if visited.insert(dep.clone()) {
                    queue.push_back(dep);
                }
            }
        }

        self.conn.execute(
            "INSERT INTO task_dependencies (task_id, depends_on) VALUES (?1, ?2)",
            params![task_id, depends_on],
        )?;
        Ok(())
    }

    pub fn remove_task_dependency(&self, task_id: &str, depends_on: &str) -> Result<bool, DbError> {
        let changed = self.conn.execute(
            "DELETE FROM task_dependencies WHERE task_id = ?1 AND depends_on = ?2",
            params![task_id, depends_on],
        )?;
        Ok(changed != 0)
    }

    pub fn list_task_dependencies(&self, task_id: &str) -> Result<Vec<String>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT depends_on FROM task_dependencies WHERE task_id = ?1 ORDER BY depends_on",
        )?;
        let rows = stmt.query_map(params![task_id], |r| r.get(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn list_task_dependents(&self, task_id: &str) -> Result<Vec<String>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id FROM task_dependencies WHERE depends_on = ?1 ORDER BY task_id",
        )?;
        let rows = stmt.query_map(params![task_id], |r| r.get(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn list_all_dependencies(&self) -> Result<Vec<(String, String)>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, depends_on FROM task_dependencies ORDER BY task_id, depends_on",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}
