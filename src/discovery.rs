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
    pub manifests: Vec<String>,
    pub test_locations: Vec<String>,
    pub architecture_boundaries: Vec<String>,
    pub unknowns_and_risks: Vec<String>,
    pub fingerprint: String,
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

const MAX_DISCOVERY_FILES: usize = 4_000;
const IGNORED_DIRECTORIES: &[&str] = &[".git", "target", "node_modules", "dist", "build"];

fn repository_files(root: &Path) -> Result<(Vec<String>, bool)> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    let mut truncated = false;
    while let Some(directory) = pending.pop() {
        let mut entries = std::fs::read_dir(&directory)
            .with_context(|| format!("failed to inspect {}", directory.display()))?
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap_or(&path);
            if entry.file_type()?.is_dir() {
                let name = entry.file_name();
                if IGNORED_DIRECTORIES.iter().any(|ignored| name == *ignored)
                    || relative.starts_with(".orc/worktrees")
                {
                    continue;
                }
                pending.push(path);
            } else if entry.file_type()?.is_file() {
                let relative = relative.to_string_lossy().replace('\\', "/");
                if crate::git::is_runtime_artifact(&relative) {
                    continue;
                }
                files.push(relative);
                if files.len() == MAX_DISCOVERY_FILES {
                    truncated = true;
                    pending.clear();
                    break;
                }
            }
        }
    }
    files.sort();
    Ok((files, truncated))
}

fn snapshot_fingerprint(
    root: &Path,
    commit: Option<&str>,
    changed: &[crate::git::ChangedFile],
    files: &[String],
) -> String {
    let mut hash: u64 = 14695981039346656037;
    for byte in commit
        .into_iter()
        .chain(
            changed
                .iter()
                .flat_map(|item| [item.status.as_str(), item.path.as_str()]),
        )
        .chain(files.iter().map(String::as_str))
        .flat_map(str::bytes)
    {
        hash = (hash ^ u64::from(byte)).wrapping_mul(1099511628211);
    }
    // The commit identifies tracked content. Metadata for changed/untracked
    // paths invalidates a cached snapshot without reading the whole repository
    // into memory or provider context.
    for item in changed {
        if let Ok(metadata) = std::fs::metadata(root.join(&item.path)) {
            for byte in metadata.len().to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(1099511628211);
            }
            if let Ok(modified) = metadata.modified()
                && let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                for byte in duration.as_nanos().to_le_bytes() {
                    hash = (hash ^ u64::from(byte)).wrapping_mul(1099511628211);
                }
            }
        }
    }
    format!("discovery-{hash:016x}")
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
    let (files, truncated) = repository_files(&root)?;
    let mut stack = Vec::new();
    if root.join("Cargo.toml").exists() {
        stack.push("Rust".into());
    }
    if root.join("package.json").exists() {
        stack.push("JavaScript/TypeScript".into());
    }
    let manifest_names = [
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "pom.xml",
        "build.gradle",
        "Makefile",
    ];
    let manifests = files
        .iter()
        .filter(|path| {
            path.rsplit('/')
                .next()
                .is_some_and(|name| manifest_names.contains(&name))
        })
        .cloned()
        .collect::<Vec<_>>();
    for manifest in &manifests {
        match manifest.rsplit('/').next().unwrap_or_default() {
            "pyproject.toml" => {
                if !stack.iter().any(|item| item == "Python") {
                    stack.push("Python".into())
                }
            }
            "go.mod" => stack.push("Go".into()),
            "pom.xml" | "build.gradle" => stack.push("Java/JVM".into()),
            _ => {}
        }
    }
    stack.sort();
    stack.dedup();
    let important_files = files
        .iter()
        .filter(|path| {
            let name = path.rsplit('/').next().unwrap_or_default();
            manifests.contains(path)
                || matches!(
                    name,
                    "README.md"
                        | "ENGINEERING.md"
                        | "AGENTS.md"
                        | "validation.toml"
                        | "validation.json"
                )
        })
        .take(160)
        .cloned()
        .collect();
    let entry_points = files
        .iter()
        .filter(|path| {
            matches!(
                path.as_str(),
                "src/main.rs" | "src/lib.rs" | "src/App.vue" | "index.js" | "index.ts"
            ) || path.ends_with("/main.rs")
                || path.ends_with("/lib.rs")
        })
        .take(80)
        .cloned()
        .collect();
    let test_locations = files
        .iter()
        .filter(|path| {
            path.starts_with("tests/")
                || path.contains("/tests/")
                || path.ends_with("_test.rs")
                || path.ends_with(".test.ts")
                || path.ends_with(".test.js")
        })
        .take(160)
        .cloned()
        .collect::<Vec<_>>();
    let mut source_directories = files
        .iter()
        .filter_map(|path| path.split_once('/').map(|(head, _)| head.to_owned()))
        .filter(|head| {
            matches!(
                head.as_str(),
                "src" | "src-tauri" | "tests" | "scripts" | "crates" | "packages" | "apps"
            )
        })
        .collect::<Vec<_>>();
    source_directories.sort();
    source_directories.dedup();
    let architecture_boundaries = source_directories
        .iter()
        .filter(|path| !matches!(path.as_str(), "tests" | "scripts"))
        .cloned()
        .collect::<Vec<_>>();
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
    let changed_files = crate::git::changed_files(&root)?;
    let fingerprint = snapshot_fingerprint(&root, commit.as_deref(), &changed_files, &files);
    let mut unknowns_and_risks = Vec::new();
    if truncated {
        unknowns_and_risks.push(format!(
            "repository inventory exceeded the bounded {MAX_DISCOVERY_FILES}-file discovery limit"
        ));
    }
    if changed_files.is_empty() {
        unknowns_and_risks.push("no uncommitted repository changes observed".into());
    } else {
        unknowns_and_risks.push(format!(
            "{} uncommitted path(s) may affect planning",
            changed_files.len()
        ));
    }
    Ok(ProjectDiscoverySnapshot {
        repository: RepositorySnapshot {
            root: root.display().to_string(),
            branch,
            commit,
            changed_files,
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
        manifests,
        test_locations,
        architecture_boundaries,
        unknowns_and_risks,
        fingerprint,
        validation_commands: crate::validation::ValidationConfig::load(&root)?.commands,
        task_state,
    })
}

/// Persist the reusable discovery artifact at the explicit orchestration
/// boundary. The underlying repository inspection remains read-only.
pub fn discover_and_persist(start: impl AsRef<Path>) -> Result<ProjectDiscoverySnapshot> {
    let root = repository_root(start)?;
    let snapshot = build_snapshot(&root)?;
    let db = Database::open(root.join(".orc/orc.db"))?;
    let project_id = db.get_project_id()?.context("no adopted project found")?;
    db.store_discovery_snapshot(project_id, &snapshot)?;
    Ok(snapshot)
}

/// Return the persisted artifact when the repository fingerprint still
/// matches; otherwise return a fresh read-only snapshot without silently
/// writing project state.
pub fn snapshot_for_provider(start: impl AsRef<Path>) -> Result<ProjectDiscoverySnapshot> {
    let root = repository_root(start)?;
    let current = build_snapshot(&root)?;
    let db = Database::open(root.join(".orc/orc.db"))?;
    let project_id = db.get_project_id()?.context("no adopted project found")?;
    let mut snapshot = db
        .load_discovery_snapshot(project_id, &current.fingerprint)?
        .unwrap_or(current);
    // The engineering contract is already a first-class field in Lead and
    // Planner packets. Keep it in the persisted full snapshot, but never nest
    // a second full copy into provider context.
    snapshot.project.engineering_contract = None;
    snapshot.repository.changed_files.truncate(200);
    snapshot.important_files.truncate(120);
    snapshot.test_locations.truncate(120);
    Ok(snapshot)
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
