use anyhow::{Context, Result};
use std::path::Path;

use crate::adoption::repository_root;
use crate::protocol::{PROTOCOL_VERSION, ProjectDiscoveryRequest, ProjectDiscoveryResponse};
use crate::storage::Database;

/// A read-only, point-in-time view of the information available to planning
/// and leadership workflows. It deliberately contains observations rather
/// than inferred recommendations.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectDiscoverySnapshot {
    pub repository: RepositorySnapshot,
    pub project: ProjectMetadata,
    pub architecture: ArchitectureSnapshot,
    pub technology_stack: Vec<String>,
    pub important_files: Vec<String>,
    pub validation_commands: Vec<String>,
    pub task_state: crate::protocol::PlanningProjectState,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RepositorySnapshot {
    pub root: String,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub changed_files: Vec<crate::git::ChangedFile>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProjectMetadata {
    pub name: String,
    pub description: Option<String>,
    pub engineering_contract: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ArchitectureSnapshot {
    pub entry_points: Vec<String>,
    pub source_directories: Vec<String>,
}

/// Collect the current repository and persisted project state without writing
/// files, changing the database, or invoking validation commands.
pub fn build_snapshot(start: impl AsRef<Path>) -> Result<ProjectDiscoverySnapshot> {
    let root = repository_root(start)?;
    let db =
        Database::open(root.join(".orc/orc.db")).context("failed to open adopted Orc database")?;
    let task_state = db
        .planning_project_state()
        .context("failed to read persisted task state")?;
    let name = db
        .get_project_name()?
        .or_else(|| root.file_name().and_then(|v| v.to_str()).map(str::to_owned))
        .context("repository root has no usable project name")?;
    let contract = std::fs::read_to_string(root.join(".orc/engineering.md")).ok();
    let mut stack = Vec::new();
    if root.join("Cargo.toml").exists() {
        stack.push("Rust".into());
    }
    if root.join("package.json").exists() {
        stack.push("JavaScript/TypeScript".into());
    }
    let important_files = [
        "README.md",
        "Cargo.toml",
        "package.json",
        ".orc/engineering.md",
        ".orc/validation.toml",
        ".orc/validation.json",
    ]
    .into_iter()
    .filter(|file| root.join(file).is_file())
    .map(str::to_owned)
    .collect();
    let entry_points = [
        "src/main.rs",
        "src/lib.rs",
        "src/App.vue",
        "index.js",
        "index.ts",
    ]
    .into_iter()
    .filter(|file| root.join(file).is_file())
    .map(str::to_owned)
    .collect();
    let source_directories = ["src", "tests", "scripts"]
        .into_iter()
        .filter(|dir| root.join(dir).is_dir())
        .map(str::to_owned)
        .collect();
    let description = root
        .join("Cargo.toml")
        .is_file()
        .then(|| {
            std::fs::read_to_string(root.join("Cargo.toml"))
                .ok()
                .and_then(|text| {
                    text.lines()
                        .find(|line| line.trim_start().starts_with("description ="))
                        .map(|line| {
                            line.split_once('=')
                                .map(|(_, value)| value.trim().trim_matches('"').to_owned())
                                .unwrap_or_default()
                        })
                })
        })
        .flatten();
    let (branch, commit) = crate::git::repository_identity(&root)?;
    Ok(ProjectDiscoverySnapshot {
        repository: RepositorySnapshot {
            root: root.display().to_string(),
            branch,
            commit,
            changed_files: crate::git::changed_files(&root)?,
        },
        project: ProjectMetadata {
            name,
            description,
            engineering_contract: contract,
        },
        architecture: ArchitectureSnapshot {
            entry_points,
            source_directories,
        },
        technology_stack: stack,
        important_files,
        validation_commands: crate::validation::ValidationConfig::load(&root)?.commands,
        task_state,
    })
}

pub fn build_request(start: impl AsRef<Path>) -> Result<ProjectDiscoveryRequest> {
    let root = repository_root(start)?;
    let engineering_contract = std::fs::read_to_string(root.join(".orc/engineering.md"))
        .context("failed to read .orc/engineering.md; run `orc adopt` first")?;
    Ok(ProjectDiscoveryRequest {
        protocol_version: PROTOCOL_VERSION,
        project_name: root
            .file_name()
            .and_then(|name| name.to_str())
            .context("repository root has no usable directory name")?
            .to_owned(),
        repository_path: root.display().to_string(),
        engineering_contract,
        instructions: "Inspect the repository rooted at repository_path. Do not modify files. Do not run destructive commands. Infer architecture conservatively from the repository. Report unknowns instead of inventing answers. Return only the structured ProjectDiscoveryResponse format.".to_owned(),
    })
}

pub fn apply_response(start: impl AsRef<Path>, response: &ProjectDiscoveryResponse) -> Result<()> {
    response.validate()?;
    let root = repository_root(start)?;
    let db_path = root.join(".orc/orc.db");
    let db = Database::open(&db_path).context("failed to open adopted Orc database")?;
    let project_id = db
        .get_project_id()?
        .context("no adopted project found; run `orc adopt` first")?;
    db.store_discovery_facts(project_id, response)?;
    write_summary(&root.join(".orc/project.md"), &render_project(response))?;
    write_summary(
        &root.join(".orc/architecture.md"),
        &render_architecture(response),
    )?;
    write_summary(&root.join(".orc/roadmap.md"), &render_roadmap(response))?;
    Ok(())
}

fn write_summary(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))
}

fn lines(values: &[String]) -> String {
    if values.is_empty() {
        "- None reported".to_owned()
    } else {
        values.iter().map(|value| format!("- {value}\n")).collect()
    }
}

fn render_project(response: &ProjectDiscoveryResponse) -> String {
    format!(
        "# Project\n\n## Name\n{}\n\n## Purpose\n{}\n\n## Languages\n{}",
        response.project.name,
        response.project.purpose,
        lines(&response.project.languages)
    )
}

fn render_architecture(response: &ProjectDiscoveryResponse) -> String {
    format!(
        "# Architecture\n\n## Entry points\n{}\n## Modules\n{}\n## Boundaries\n{}",
        lines(&response.architecture.entry_points),
        lines(&response.architecture.modules),
        lines(&response.architecture.boundaries)
    )
}

fn render_roadmap(response: &ProjectDiscoveryResponse) -> String {
    format!(
        "# Roadmap\n\nThis records discovered current state only; it does not create future tasks.\n\n## Implemented\n{}\n## In progress\n{}\n## Risks\n{}\n## Unknowns\n{}",
        lines(&response.state.implemented),
        lines(&response.state.in_progress),
        lines(&response.state.risks),
        lines(&response.unknowns)
    )
}

pub fn parse_response(data: &str) -> Result<ProjectDiscoveryResponse> {
    let response: ProjectDiscoveryResponse =
        serde_json::from_str(data).context("failed to parse project discovery response JSON")?;
    response.validate()?;
    Ok(response)
}
