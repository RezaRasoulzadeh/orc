use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::storage::Database;

const PROJECT_TEMPLATE: &str =
    "# Project\n\nThis document is populated by `orc apply-discovery`.\n";
const ARCHITECTURE_TEMPLATE: &str =
    "# Architecture\n\nThis document is populated by `orc apply-discovery`.\n";
const ROADMAP_TEMPLATE: &str =
    "# Roadmap\n\nCurrent discovered state, risks, and unknowns will be recorded here.\n";

pub fn repository_root(start: impl AsRef<Path>) -> Result<PathBuf> {
    let start = start.as_ref();
    let output = Command::new("git")
        .current_dir(start)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("failed to check whether the directory is inside a Git repository")?;
    if !output.status.success() {
        bail!("current directory is not inside a Git repository");
    }
    let root = String::from_utf8(output.stdout)
        .context("Git returned a non-UTF-8 repository path")?
        .trim()
        .to_owned();
    if root.is_empty() {
        bail!("Git did not report a repository root");
    }
    std::fs::canonicalize(root).context("failed to resolve Git repository root")
}

pub fn adopt(start: impl AsRef<Path>) -> Result<PathBuf> {
    let root = repository_root(start)?;
    let orc_dir = root.join(".orc");
    let db_path = orc_dir.join("orc.db");

    if db_path.exists() {
        let db = Database::open(&db_path).with_context(|| {
            format!(
                "{}.orc/orc.db already exists but is not a valid Orc database",
                root.display()
            )
        })?;
        if db.get_project_id()?.is_some() {
            ensure_adoption_files(&orc_dir)?;
            return Ok(root);
        }
        bail!(
            "{}.orc/orc.db already exists and the project is not cleanly adopted",
            root.display()
        );
    }

    std::fs::create_dir_all(&orc_dir)
        .with_context(|| format!("failed to create {}", orc_dir.display()))?;
    let db = Database::init(&db_path).context("failed to initialize Orc database")?;
    let project_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .context("repository root has no usable directory name")?;
    db.create_project(project_name)
        .context("failed to create adopted project record")?;

    ensure_adoption_files(&orc_dir)?;
    Ok(root)
}

/// Adopt a repository and assess the operator's objective with the read-only
/// Lead. The Lead context includes the current structured discovery snapshot;
/// no proposal is applied and no work is dispatched by this operation.
pub fn adopt_and_invoke_lead(
    start: impl AsRef<Path>,
    objective: &str,
    backend: &dyn crate::lead::LeadBackend,
    context_limit: usize,
) -> Result<(PathBuf, crate::lead::LeadResponse)> {
    let root = adopt(start)?;
    let db =
        Database::open(root.join(".orc/orc.db")).context("failed to open adopted Orc database")?;
    let response = crate::lead::LeadService::new_with_required_discovery(&db, &root).invoke(
        objective,
        backend,
        context_limit,
    )?;
    if response.decision.is_none() {
        bail!("Lead returned no decision for adoption objective");
    }
    Ok((root, response))
}

/// Provide the built-in Codex Lead when an objective is supplied on a first adoption.
/// The CLI still permits an operator to replace this configuration afterwards.
pub fn ensure_default_lead(db: &Database) -> Result<()> {
    if db.lead_provider_config()?.is_some() {
        return Ok(());
    }
    if db.get_agent("codex-main")?.is_none() {
        db.insert_agent(&crate::registry::AgentDefinition {
            id: "codex-main".into(),
            display_name: "Codex Main".into(),
            backend: "codex".into(),
            execution_mode: crate::registry::AUTOMATED.into(),
            enabled: true,
            priority: 0,
            capabilities: vec![],
            status: crate::registry::AVAILABLE.into(),
            unavailable_reason: None,
            profile_path: None,
            model: None,
            reasoning_effort: None,
            config_metadata: None,
            quota_remaining_percent: None,
            quota_reset_at: None,
            quota_checked_at: None,
            quota_source: None,
            quota_limits: None,
            actions: vec![crate::registry::AgentAction::Lead],
        })?;
    }
    if let Some(project_id) = db.get_project_id()? {
        db.reference_global_agent(project_id, "codex-main")?;
    }
    db.set_lead_provider_config(&crate::lead::LeadProviderConfig {
        agent_id: "codex-main".into(),
        model: None,
        reasoning_effort: None,
    })?;
    Ok(())
}

pub fn ensure_adoption_files(orc_dir: &Path) -> Result<()> {
    ensure_contract_file(
        &orc_dir.join("engineering.md"),
        crate::contract::DEFAULT_ENGINEERING_CONTRACT,
    )?;
    ensure_file(&orc_dir.join("project.md"), PROJECT_TEMPLATE)?;
    ensure_file(&orc_dir.join("architecture.md"), ARCHITECTURE_TEMPLATE)?;
    ensure_file(&orc_dir.join("roadmap.md"), ROADMAP_TEMPLATE)?;
    Ok(())
}

fn ensure_file(path: &Path, contents: &str) -> Result<()> {
    if path.exists() {
        if !path.is_file() {
            bail!("{} exists but is not a file", path.display());
        }
        if std::fs::metadata(path)?.len() != 0 {
            return Ok(());
        }
    }
    std::fs::write(path, contents)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn ensure_contract_file(path: &Path, contents: &str) -> Result<()> {
    if path.exists() {
        if !path.is_file() {
            bail!("{} exists but is not a file", path.display());
        }
        return Ok(());
    }
    std::fs::write(path, contents)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}
