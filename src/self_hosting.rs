use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const PROJECT_IDENTITY_PATH: &str = ".orc/project-identity.json";
pub const ORC_REPOSITORY_ID: &str = "dev.orc.orchestrator";
pub const SELF_HOSTED_MUTATION_ENV: &str = "ORC_SELF_HOSTED_MUTATION";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryIdentity {
    pub schema_version: u32,
    pub repository_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfHostingReadinessState {
    NotApplicable,
    Ready,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfHostingReadiness {
    pub recognized: bool,
    pub repository_id: Option<String>,
    pub state: SelfHostingReadinessState,
    pub blocking_guards: Vec<String>,
}

impl SelfHostingReadiness {
    pub fn blocks_execution(&self) -> bool {
        self.state == SelfHostingReadinessState::Blocked
    }
}

/// Inspect source-controlled repository identity and the checkout shape without
/// relying on a remote URL or an absolute installation path.
pub fn inspect(repository: impl AsRef<Path>) -> SelfHostingReadiness {
    let repository = repository.as_ref();
    let working = std::fs::read_to_string(repository.join(PROJECT_IDENTITY_PATH)).ok();
    let committed = git_output(
        repository,
        &["show", &format!("HEAD:{PROJECT_IDENTITY_PATH}")],
    )
    .ok();
    let working_identity = working
        .as_deref()
        .and_then(|value| serde_json::from_str::<RepositoryIdentity>(value).ok());
    let committed_identity = committed
        .as_deref()
        .and_then(|value| serde_json::from_str::<RepositoryIdentity>(value).ok());
    let recognized = working_identity
        .as_ref()
        .is_some_and(|identity| identity.repository_id == ORC_REPOSITORY_ID)
        || committed_identity
            .as_ref()
            .is_some_and(|identity| identity.repository_id == ORC_REPOSITORY_ID);
    let repository_id = if recognized {
        Some(ORC_REPOSITORY_ID.into())
    } else {
        working_identity
            .as_ref()
            .or(committed_identity.as_ref())
            .map(|identity| identity.repository_id.clone())
    };

    if !recognized {
        return SelfHostingReadiness {
            recognized: false,
            repository_id,
            state: SelfHostingReadinessState::NotApplicable,
            blocking_guards: Vec::new(),
        };
    }

    let mut blocking_guards = Vec::new();
    match working_identity.as_ref() {
        Some(identity)
            if identity.schema_version == 1 && identity.repository_id == ORC_REPOSITORY_ID => {}
        Some(_) => blocking_guards.push(format!(
            "{PROJECT_IDENTITY_PATH} does not contain the supported Orc repository identity"
        )),
        None => blocking_guards.push(format!(
            "{PROJECT_IDENTITY_PATH} is missing or invalid in the working tree"
        )),
    }
    match committed_identity.as_ref() {
        Some(identity)
            if identity.schema_version == 1 && identity.repository_id == ORC_REPOSITORY_ID => {}
        _ => blocking_guards.push(format!(
            "{PROJECT_IDENTITY_PATH} must be committed with the supported Orc repository identity"
        )),
    }

    let canonical_repository = repository.canonicalize().ok();
    let top_level = git_output(repository, &["rev-parse", "--show-toplevel"])
        .ok()
        .and_then(|path| PathBuf::from(path.trim()).canonicalize().ok());
    if canonical_repository.is_none() || top_level != canonical_repository {
        blocking_guards.push(
            "the configured project root is not the main repository top-level checkout".into(),
        );
    }
    if !repository.join(".git").is_dir() {
        blocking_guards.push(
            "the configured project root is a linked worktree rather than the main checkout".into(),
        );
    }

    SelfHostingReadiness {
        recognized: true,
        repository_id: Some(ORC_REPOSITORY_ID.into()),
        state: if blocking_guards.is_empty() {
            SelfHostingReadinessState::Ready
        } else {
            SelfHostingReadinessState::Blocked
        },
        blocking_guards,
    }
}

pub fn ensure_execution_ready(repository: impl AsRef<Path>) -> Result<()> {
    let readiness = inspect(repository);
    if readiness.blocks_execution() {
        bail!(
            "self-hosting readiness guards block execution: {}",
            readiness.blocking_guards.join("; ")
        );
    }
    Ok(())
}

pub fn is_orc_repository(repository: impl AsRef<Path>) -> bool {
    inspect(repository).recognized
}

pub fn mark_mutation_process(command: &mut Command, repository: impl AsRef<Path>) {
    if is_orc_repository(repository) {
        command.env(SELF_HOSTED_MUTATION_ENV, "1");
    }
}

pub fn reject_recursive_invocation() -> Result<()> {
    if std::env::var_os(SELF_HOSTED_MUTATION_ENV).is_some() {
        bail!("recursive Orc invocation is disabled inside a self-hosted task mutation process");
    }
    Ok(())
}

fn git_output(repository: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(repository)
        .args(args)
        .output()
        .with_context(|| format!("failed to execute git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!("git {} failed", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
