use crate::task::{Task, TaskPriority, TaskStatus};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};
use std::{io, path::Path};

#[derive(Debug, Clone)]
pub struct AgentRun {
    pub id: i64,
    pub project_id: i64,
    pub task_id: Option<String>,
    pub agent: String,
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
    #[error("invalid next task id in database: {0}")]
    InvalidSequence(String),
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
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id),
                title TEXT NOT NULL,
                objective TEXT NOT NULL,
                role TEXT NOT NULL,
                priority TEXT NOT NULL,
                status TEXT NOT NULL,
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
        Ok(Self { conn })
    }

    fn configure(conn: &Connection) -> Result<(), DbError> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
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
        Ok(Task {
            id: row.get(0)?,
            title: row.get(1)?,
            objective: row.get(2)?,
            role: row.get(3)?,
            priority: priority_value,
            status: status_value,
        })
    }

    pub fn list_tasks(&self) -> Result<Vec<Task>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, objective, role, priority, status FROM tasks ORDER BY created_at",
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
                "SELECT id, title, objective, role, priority, status FROM tasks WHERE id = ?1",
                params![id],
                Self::task_from_row,
            )
            .optional()?)
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
            status: row.get(4)?,
            output: row.get(5)?,
            started_at: row.get(6)?,
            finished_at: row.get(7)?,
        })
    }

    pub fn create_agent_run(
        &self,
        project_id: i64,
        task_id: &str,
        agent: &str,
    ) -> Result<i64, DbError> {
        self.conn.execute(
            "INSERT INTO agent_runs (project_id, task_id, agent, status, started_at) VALUES (?1, ?2, ?3, 'running', CURRENT_TIMESTAMP)",
            params![project_id, task_id, agent],
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

    pub fn list_agent_runs(&self, project_id: i64, limit: usize) -> Result<Vec<AgentRun>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, task_id, agent, status, output, started_at, finished_at FROM agent_runs WHERE project_id = ?1 ORDER BY started_at DESC LIMIT ?2",
        )?;
        Ok(stmt
            .query_map(params![project_id, limit as i64], Self::agent_run_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn list_agent_runs_for_task(&self, task_id: &str) -> Result<Vec<AgentRun>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, task_id, agent, status, output, started_at, finished_at FROM agent_runs WHERE task_id = ?1 ORDER BY started_at DESC, id DESC",
        )?;
        Ok(stmt
            .query_map(params![task_id], Self::agent_run_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn store_worktree_metadata(
        &self,
        agent_run_id: i64,
        task_id: &str,
        branch_name: &str,
        worktree_path: &str,
    ) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO worktree_metadata (agent_run_id, task_id, branch_name, worktree_path) VALUES (?1, ?2, ?3, ?4)",
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
}
