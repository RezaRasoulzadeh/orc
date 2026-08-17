use anyhow::{Context, Result};
use std::path::Path;

use crate::adoption::repository_root;
use crate::protocol::{PROTOCOL_VERSION, ProjectDiscoveryRequest, ProjectDiscoveryResponse};
use crate::storage::Database;

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
