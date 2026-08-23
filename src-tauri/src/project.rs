use anyhow::{Context, Result, bail};
use orc::app::OrcApp;
use orc::events::EventSubscription;
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
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
    #[serde(default = "default_project_status")]
    pub status: ProjectStatus,
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
    pub status: ProjectStatus,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProjectStatus {
    Available,
    Missing,
    Invalid,
    TemporarilyUnavailable,
}

fn default_project_status() -> ProjectStatus {
    ProjectStatus::Available
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

    pub fn take_subscription(&mut self) -> Result<(EventSubscription, Arc<AtomicBool>)> {
        let subscription = self
            .subscription
            .take()
            .context("project session subscription is unavailable")?;
        Ok((subscription, self.cancellation.clone()))
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
                let availability = self.inspect(&project.id);
                project.available = availability.status == ProjectStatus::Available;
                project.status = availability.status;
                project
            })
            .collect()
    }

    pub fn availability(&self, id: &str) -> Result<ProjectAvailability> {
        let project = self.file.projects.iter().find(|project| project.id == id);
        let result = match project {
            Some(project) => self.inspect(&project.id),
            None => bail!("registered project '{id}' not found"),
        };
        Ok(ProjectAvailability {
            project_id: id.to_string(),
            available: result.status == ProjectStatus::Available,
            status: result.status,
            error: result.error,
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
            status: ProjectStatus::Available,
        };
        self.file.projects.push(project.clone());
        self.save()?;
        Ok(project)
    }

    pub fn remove(&mut self, id: &str) -> Result<bool> {
        let old = self.file.projects.len();
        self.file.projects.retain(|project| project.id != id);
        let removed = old != self.file.projects.len();
        if removed {
            self.save()?;
        }
        Ok(removed)
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

    pub fn relocate(&mut self, id: &str, root: impl AsRef<Path>) -> Result<RegisteredProject> {
        let root = root.as_ref().canonicalize().with_context(|| {
            format!(
                "canonicalize replacement project root {}",
                root.as_ref().display()
            )
        })?;
        let project = self
            .file
            .projects
            .iter_mut()
            .find(|project| project.id == id)
            .context("registered project not found")?;
        let identity = read_identity(&root)?;
        if identity != (project.project_id, project.project_name.clone()) {
            bail!("replacement project identity does not match registered project");
        }
        project.repository_root = root;
        project.available = true;
        project.status = ProjectStatus::Available;
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
        let identity = read_identity(&project.repository_root)?;
        if identity != (project.project_id, project.project_name.clone()) {
            bail!("project database identity does not match registered project");
        }
        Ok(())
    }

    fn inspect(&self, id: &str) -> ProjectAvailability {
        let path = self
            .file
            .projects
            .iter()
            .find(|project| project.id == id)
            .map(|project| project.repository_root.clone());
        if let Some(path) = path {
            if !path.exists() {
                return ProjectAvailability {
                    project_id: id.to_string(),
                    available: false,
                    status: ProjectStatus::Missing,
                    error: Some(format!(
                        "repository root does not exist: {}",
                        path.display()
                    )),
                };
            }
            if std::fs::metadata(&path).is_err() {
                return ProjectAvailability {
                    project_id: id.to_string(),
                    available: false,
                    status: ProjectStatus::TemporarilyUnavailable,
                    error: Some(format!(
                        "repository root is temporarily unavailable: {}",
                        path.display()
                    )),
                };
            }
        }
        let result = self.validate(id);
        let status = match &result {
            Ok(()) => ProjectStatus::Available,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == ErrorKind::NotFound) =>
            {
                ProjectStatus::Missing
            }
            Err(error)
                if error.to_string().contains("identity does not match")
                    || error.to_string().contains("database") =>
            {
                ProjectStatus::Invalid
            }
            Err(_) => ProjectStatus::TemporarilyUnavailable,
        };
        ProjectAvailability {
            project_id: id.to_string(),
            available: status == ProjectStatus::Available,
            status,
            error: result.err().map(|e| e.to_string()),
        }
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_vec_pretty(&self.file)?)?;
        Ok(())
    }
}

fn read_identity(root: &Path) -> Result<(i64, String)> {
    if !root.is_dir() {
        bail!("repository root is not a directory: {}", root.display());
    }
    let db = root.join(".orc/orc.db");
    if !db.is_file() {
        bail!("project database not found at {}", db.display());
    }
    let database = orc::Database::open(&db).context("open project database")?;
    Ok((
        database
            .get_project_id()?
            .context("project database has no project")?,
        database.get_project_name()?.unwrap_or_else(|| "orc".into()),
    ))
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
        let database = orc::Database::init(root.join(".orc/orc.db")).unwrap();
        database.create_project("Demo").unwrap();
        let path = dir.path().join("registry.json");
        let mut registry = ProjectRegistry::open(&path).unwrap();
        let first = registry.register(&root, Some("Demo".into())).unwrap();
        let duplicate = registry.register(&root, None).unwrap();
        assert_eq!(first.id, duplicate.id);
        let reopened = ProjectRegistry::open(path).unwrap();
        assert_eq!(reopened.projects(), vec![first]);
    }

    #[test]
    fn removal_only_forgets_registration_and_reimport_recovers_state() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir_all(root.join(".orc")).unwrap();
        let database_path = root.join(".orc/orc.db");
        let database = orc::Database::init(&database_path).unwrap();
        database.create_project("Demo").unwrap();
        let database_before = std::fs::read(&database_path).unwrap();
        let path = dir.path().join("registry.json");
        let mut registry = ProjectRegistry::open(&path).unwrap();
        let project = registry.register(&root, Some("Demo".into())).unwrap();

        assert!(registry.remove(&project.id).unwrap());
        assert!(!registry.remove(&project.id).unwrap());
        assert!(!registry.remove("unknown").unwrap());
        assert!(registry.projects().is_empty());
        assert_eq!(std::fs::read(&database_path).unwrap(), database_before);
        assert!(root.join(".orc/orc.db").is_file());

        let recovered = registry.register(&root, None).unwrap();
        assert_eq!(recovered.project_id, project.project_id);
        assert_eq!(recovered.project_name, project.project_name);
        assert_eq!(recovered.repository_root, project.repository_root);
        assert_eq!(std::fs::read(&database_path).unwrap(), database_before);
    }
}
