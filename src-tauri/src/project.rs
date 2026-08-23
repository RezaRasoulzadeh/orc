use anyhow::{Context, Result, bail};
use orc::app::OrcApp;
use orc::events::EventSubscription;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegisteredProject {
    pub id: String,
    pub display_name: String,
    pub repository_root: PathBuf,
    pub project_id: i64,
    pub project_name: String,
    pub last_opened_at: Option<u64>,
    pub available: bool,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct RegistryFile {
    projects: Vec<RegisteredProject>,
}

pub struct ProjectRegistry {
    path: PathBuf,
    file: RegistryFile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectAvailability {
    pub project_id: String,
    pub available: bool,
    pub error: Option<String>,
}

pub struct ProjectSession {
    pub project: RegisteredProject,
    pub app: OrcApp,
    subscription: Option<EventSubscription>,
    cancellation: Arc<AtomicBool>,
}

impl ProjectSession {
    pub fn open(project: RegisteredProject) -> Result<Self> {
        let app = OrcApp::open(
            project.repository_root.join(".orc/orc.db"),
            &project.repository_root,
        )?;
        let subscription = app.subscribe();
        Ok(Self {
            project,
            app,
            subscription: Some(subscription),
            cancellation: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn take_subscription(&mut self) -> (EventSubscription, Arc<AtomicBool>) {
        (self.subscription.take().expect("session subscription"), self.cancellation.clone())
    }
}

impl Drop for ProjectSession {
    fn drop(&mut self) {
        self.cancellation.store(true, Ordering::Release);
    }
}

impl ProjectRegistry {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let file = if path.is_file() {
            serde_json::from_str(
                &std::fs::read_to_string(&path)
                    .with_context(|| format!("read project registry {}", path.display()))?,
            )?
        } else {
            RegistryFile::default()
        };
        Ok(Self { path, file })
    }

    pub fn projects(&self) -> Vec<RegisteredProject> {
        self.file
            .projects
            .iter()
            .map(|project| {
                let mut project = project.clone();
                project.available = self.validate(project.id.as_str()).is_ok();
                project
            })
            .collect()
    }

    pub fn availability(&self, id: &str) -> Result<ProjectAvailability> {
        let project = self.file.projects.iter().find(|project| project.id == id);
        let result = match project {
            Some(project) => self.validate(&project.id),
            None => Err(anyhow::anyhow!("registered project '{id}' not found")),
        };
        Ok(ProjectAvailability {
            project_id: id.to_string(),
            available: result.is_ok(),
            error: result.err().map(|error| error.to_string()),
        })
    }

    pub fn register(
        &mut self,
        root: impl AsRef<Path>,
        display_name: Option<String>,
    ) -> Result<RegisteredProject> {
        let root = root
            .as_ref()
            .canonicalize()
            .with_context(|| format!("canonicalize project root {}", root.as_ref().display()))?;
        let db = root.join(".orc/orc.db");
        if !db.is_file() {
            bail!("project database not found at {}", db.display());
        }
        let database = orc::Database::open(&db).context("open project database")?;
        let project_id = database
            .get_project_id()?
            .context("project database has no project")?;
        let project_name = database.get_project_name()?.unwrap_or_else(|| "orc".into());
        if let Some(existing) = self.file.projects.iter_mut().find(|project| {
            project.repository_root == root
                || (project.project_id == project_id && project.project_name == project_name)
        }) {
            let changed = existing.repository_root != root;
            if existing.repository_root != root {
                existing.repository_root = root;
                existing.available = true;
            }
            let result = existing.clone();
            if changed {
                self.save()?;
            }
            return Ok(result);
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let project = RegisteredProject {
            id: format!("desktop-{now:x}"),
            display_name: display_name.unwrap_or_else(|| project_name.clone()),
            repository_root: root,
            project_id,
            project_name,
            last_opened_at: None,
            available: true,
        };
        self.file.projects.push(project.clone());
        self.save()?;
        Ok(project)
    }

    pub fn remove(&mut self, id: &str) -> Result<bool> {
        let old = self.file.projects.len();
        self.file.projects.retain(|project| project.id != id);
        if old != self.file.projects.len() {
            self.save()?;
        }
        Ok(old != self.file.projects.len())
    }

    pub fn mark_opened(&mut self, id: &str) -> Result<RegisteredProject> {
        self.validate(id)?;
        let project = self
            .file
            .projects
            .iter_mut()
            .find(|project| project.id == id)
            .context("registered project not found")?;
        project.last_opened_at = Some(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs());
        project.available = true;
        let result = project.clone();
        self.save()?;
        Ok(result)
    }

    fn validate(&self, id: &str) -> Result<()> {
        let project = self
            .file
            .projects
            .iter()
            .find(|project| project.id == id)
            .context("registered project not found")?;
        if !project.repository_root.is_dir() {
            bail!("repository root is not a directory: {}", project.repository_root.display());
        }
        let db = project.repository_root.join(".orc/orc.db");
        if !db.is_file() {
            bail!("project database not found at {}", db.display());
        }
        let database = orc::Database::open(&db).context("open project database")?;
        let project_id = database.get_project_id()?.context("project database has no project")?;
        let project_name = database.get_project_name()?.unwrap_or_else(|| "orc".into());
        if project_id != project.project_id || project_name != project.project_name {
            bail!("project database identity does not match registered project");
        }
        Ok(())
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_vec_pretty(&self.file)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn registry_reopens_and_deduplicates() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir_all(root.join(".orc")).unwrap();
        orc::Database::init(root.join(".orc/orc.db")).unwrap();
        let path = dir.path().join("registry.json");
        let mut registry = ProjectRegistry::open(&path).unwrap();
        let first = registry.register(&root, Some("Demo".into())).unwrap();
        let duplicate = registry.register(&root, None).unwrap();
        assert_eq!(first.id, duplicate.id);
        let reopened = ProjectRegistry::open(path).unwrap();
        assert_eq!(reopened.projects(), vec![first]);
    }
}
