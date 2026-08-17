use crate::registry::{AgentDefinition, QuotaLimits};
use crate::task::{Task, TaskPriority, TaskStatus};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};
use std::{io, path::Path};

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
}

pub struct Database {
    conn: Connection,
}

impl Database {
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
        Self::ensure_task_columns(&conn)?;
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
        Self::ensure_task_columns(conn)?;
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
            "INSERT INTO agents (id, backend, display_name, enabled, priority, capabilities, status, unavailable_reason, profile_path, config_metadata, execution_mode, quota_remaining_percent, quota_reset_at, quota_checked_at, quota_source, quota_limits) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
        let quota_limits_json: Option<String> = row.get(15)?;
        let quota_limits = quota_limits_json
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| {
                rusqlite::Error::InvalidParameterName(format!("invalid quota limits: {error}"))
            })?;
        Ok(AgentDefinition {
            id: row.get(0)?,
            backend: row.get(1)?,
            execution_mode: row.get(10)?,
            display_name: row.get(2)?,
            enabled: row.get::<_, i64>(3)? != 0,
            priority: row.get(4)?,
            capabilities,
            status: row.get(6)?,
            unavailable_reason: row.get(7)?,
            profile_path: row.get(8)?,
            config_metadata: row.get(9)?,
            quota_remaining_percent: row.get(11)?,
            quota_reset_at: row.get(12)?,
            quota_checked_at: row.get(13)?,
            quota_source: row.get(14)?,
            quota_limits,
        })
    }

    pub fn get_agent(&self, id: &str) -> Result<Option<AgentDefinition>, DbError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, backend, display_name, enabled, priority, capabilities, status, unavailable_reason, profile_path, config_metadata, execution_mode, quota_remaining_percent, quota_reset_at, quota_checked_at, quota_source, quota_limits FROM agents WHERE id = ?1",
                params![id],
                Self::agent_from_row,
            )
            .optional()?)
    }

    pub fn list_agents(&self) -> Result<Vec<AgentDefinition>, DbError> {
        let mut statement = self.conn.prepare(
            "SELECT id, backend, display_name, enabled, priority, capabilities, status, unavailable_reason, profile_path, config_metadata, execution_mode, quota_remaining_percent, quota_reset_at, quota_checked_at, quota_source, quota_limits FROM agents ORDER BY id",
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
        Ok(Task {
            id: row.get(0)?,
            title: row.get(1)?,
            objective: row.get(2)?,
            role: row.get(3)?,
            priority: priority_value,
            status: status_value,
            required_capabilities,
        })
    }

    pub fn list_tasks(&self) -> Result<Vec<Task>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, objective, role, priority, status, required_capabilities FROM tasks ORDER BY created_at",
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
            self.conn.execute("INSERT INTO tasks (id, project_id, title, objective, role, priority, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'backlog')", params![id, project_id, title, objective, role, priority_str])?;
            self.conn.execute(
                "UPDATE meta SET value = ?1 WHERE key = 'next_task_id'",
                params![(seq + 1).to_string()],
            )?;
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
                            "INSERT INTO tasks (id, project_id, title, objective, role, priority, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'backlog')",
                            params![id, project_id, title, objective, role, priority_str],
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
                "SELECT id, title, objective, role, priority, status, required_capabilities FROM tasks WHERE id = ?1",
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

    #[allow(dead_code)]
    pub fn update_task_status(&self, id: &str, status: TaskStatus) -> Result<bool, DbError> {
        let changed = self.conn.execute(
            "UPDATE tasks SET status = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![status.to_string(), id],
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
    pub fn list_approval_requests(&self, project_id: i64) -> Result<Vec<String>, DbError> {
        let mut stmt = self
            .conn
            .prepare("SELECT reason FROM approval_requests WHERE project_id = ?1 ORDER BY id")?;
        Ok(stmt
            .query_map(params![project_id], |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()?)
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
            "INSERT INTO agent_runs (project_id, task_id, agent, execution_mode, status, started_at) VALUES (?1, ?2, ?3, ?4, 'running', CURRENT_TIMESTAMP)",
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
            "UPDATE agent_runs SET status = ?1, output = ?2, finished_at = CURRENT_TIMESTAMP WHERE id = ?3",
            params![status, output, run_id],
        )?;
        Ok(changed != 0)
    }

    pub fn set_agent_run_waiting_external(&self, run_id: i64) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "UPDATE agent_runs SET status = 'waiting_external' WHERE id = ?1 AND status = 'running'",
            params![run_id],
        )? != 0)
    }

    pub fn get_agent_run(&self, run_id: i64) -> Result<Option<AgentRun>, DbError> {
        Ok(self.conn.query_row(
            "SELECT id, project_id, task_id, agent, execution_mode, status, output, started_at, finished_at FROM agent_runs WHERE id = ?1",
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
        Ok(task_id)
    }

    pub fn list_agent_runs(&self, project_id: i64, limit: usize) -> Result<Vec<AgentRun>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, task_id, agent, execution_mode, status, output, started_at, finished_at FROM agent_runs WHERE project_id = ?1 ORDER BY started_at DESC LIMIT ?2",
        )?;
        Ok(stmt
            .query_map(params![project_id, limit as i64], Self::agent_run_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn list_agent_runs_for_task(&self, task_id: &str) -> Result<Vec<AgentRun>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, task_id, agent, execution_mode, status, output, started_at, finished_at FROM agent_runs WHERE task_id = ?1 ORDER BY started_at DESC, id DESC",
        )?;
        Ok(stmt
            .query_map(params![task_id], Self::agent_run_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn update_agent_run_output(&self, run_id: i64, output: &str) -> Result<bool, DbError> {
        let changed = self.conn.execute(
            "UPDATE agent_runs SET output = ?1 WHERE id = ?2",
            params![output, run_id],
        )?;
        Ok(changed != 0)
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
