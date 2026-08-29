use crate::worker::{DEFAULT_VALIDATION_TIMEOUT, configured_timeout, run_command_with_timeout};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationConfig {
    pub commands: Vec<String>,
    #[serde(default)]
    pub groups: Vec<ValidationGroup>,
    #[serde(default)]
    pub boundaries: Vec<ValidationBoundary>,
}

/// A named, deterministically selectable subset of the configured validation
/// commands. Groups let review pick the smallest authoritative set of
/// commands for a task's affected subsystem instead of running every
/// configured command for every task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationGroup {
    pub name: String,
    pub commands: Vec<String>,
}

/// An explicit override for a single path that legitimately crosses more
/// than one validation group's boundary - e.g. the Tauri command surface
/// consumed by the frontend, or a hand-mirrored shared type/schema file -
/// where a single-group path heuristic would be unsafe. A changed file
/// matching `path` contributes every listed group instead of the one
/// `group_for_path` would otherwise infer. This is deliberately explicit,
/// per-project data rather than an attempt to infer cross-boundary impact
/// from file contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationBoundary {
    pub path: String,
    pub groups: Vec<String>,
}

/// The outcome of selecting task-specific validation commands: which
/// commands to run, and why, so review can persist and later explain its
/// selection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationSelection {
    pub commands: Vec<String>,
    pub groups: Vec<String>,
    pub rationale: Vec<String>,
}

/// Classify a single changed file path into the validation group whose
/// commands protect it. This is a small, principled rule set based on file
/// extension and top-level directory - not a per-file mapping table.
fn group_for_path(path: &str) -> Option<&'static str> {
    let path = path.split_once(" -> ").map_or(path, |(_, dst)| dst);
    if path.starts_with("src-tauri/") {
        return Some("tauri");
    }
    if path.starts_with("packaging/")
        || path == "scripts/validate-package.mjs"
        || path == "scripts/tauri-build.mjs"
    {
        return Some("packaging");
    }
    if path.ends_with(".rs") || path == "Cargo.toml" || path == "Cargo.lock" {
        return Some("rust-core");
    }
    if path.ends_with(".vue")
        || path.ends_with(".ts")
        || path.ends_with(".tsx")
        || path.ends_with(".js")
        || path.ends_with(".mjs")
        || path.ends_with(".css")
        || path == "package.json"
        || path == "package-lock.json"
        || path == "tsconfig.json"
        || path == "vite.config.ts"
        || path == "index.html"
    {
        return Some("frontend");
    }
    None
}

impl ValidationConfig {
    /// All commands known to this configuration, across every group and the
    /// flat command list, in a stable de-duplicated order.
    pub fn known_commands(&self) -> Vec<String> {
        let mut seen = std::collections::BTreeSet::new();
        let mut ordered = Vec::new();
        for command in self
            .commands
            .iter()
            .chain(self.groups.iter().flat_map(|group| group.commands.iter()))
        {
            if seen.insert(command.clone()) {
                ordered.push(command.clone());
            }
        }
        ordered
    }

    fn group(&self, name: &str) -> Option<&ValidationGroup> {
        self.groups.iter().find(|group| group.name == name)
    }

    /// Classify a single changed file into the validation group(s) whose
    /// commands protect it. An explicit `[[boundaries]]` entry (see
    /// [`ValidationBoundary`]) takes precedence and can contribute more than
    /// one group; otherwise falls back to the path/extension heuristic in
    /// [`group_for_path`], which only ever contributes one.
    fn groups_for_path(&self, path: &str) -> Vec<&str> {
        let normalized = path.split_once(" -> ").map_or(path, |(_, dst)| dst);
        if let Some(boundary) = self
            .boundaries
            .iter()
            .find(|boundary| boundary.path == normalized)
        {
            return boundary.groups.iter().map(String::as_str).collect();
        }
        group_for_path(normalized).into_iter().collect()
    }

    /// Select the smallest authoritative set of configured validation
    /// commands relevant to a task, given its changed files and any
    /// explicit task-required validation commands.
    ///
    /// Files that cannot be classified into a known group are treated as
    /// unclassified. If every changed file is unclassified, or a mix of
    /// unrelated groups plus unclassified files is present, selection
    /// conservatively escalates to the broader "integration" group (or, if
    /// none is configured, every known command) rather than guessing.
    pub fn select_for_task(
        &self,
        changed_files: &[String],
        required: &[String],
    ) -> ValidationSelection {
        let mut selection = ValidationSelection::default();
        if self.groups.is_empty() {
            // No group taxonomy configured: fall back to the flat command
            // list, which is the only relevance signal available.
            selection.commands = self.commands.clone();
            if !selection.commands.is_empty() {
                selection.rationale.push(
                    "no validation groups configured; using the configured command list".into(),
                );
            }
        } else {
            let mut matched_groups = std::collections::BTreeSet::new();
            let mut unclassified = false;
            for file in changed_files {
                let groups = self.groups_for_path(file);
                if groups.is_empty() {
                    unclassified = true;
                }
                for name in groups {
                    if self.group(name).is_some() {
                        matched_groups.insert(name);
                    } else {
                        // A boundary (or, in principle, a future heuristic)
                        // named a group that isn't configured. Selection
                        // cannot be narrowed safely.
                        unclassified = true;
                    }
                }
            }
            if unclassified || matched_groups.is_empty() {
                if let Some(integration) = self.group("integration") {
                    selection.groups.push(integration.name.clone());
                    selection.rationale.push(
                        "task touches files outside the known validation groups; selected the broader integration group"
                            .into(),
                    );
                    selection
                        .commands
                        .extend(integration.commands.iter().cloned());
                } else {
                    selection.rationale.push(
                        "task touches files outside the known validation groups and no integration group is configured; using every known command"
                            .into(),
                    );
                    selection.commands = self.known_commands();
                }
            } else {
                for name in &matched_groups {
                    if let Some(group) = self.group(name) {
                        selection.groups.push(group.name.clone());
                        selection.rationale.push(format!(
                            "changed files matched the '{name}' validation group"
                        ));
                        selection.commands.extend(group.commands.iter().cloned());
                    }
                }
            }
        }
        let known = self.known_commands();
        for command in required {
            let command = command.trim();
            if command.is_empty() {
                continue;
            }
            if known.iter().any(|known| known == command) {
                selection.rationale.push(format!(
                    "'{command}' is an explicit task-required validation command"
                ));
                selection.commands.push(command.to_owned());
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        selection
            .commands
            .retain(|command| seen.insert(command.clone()));
        selection
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationStepResult {
    pub command: String,
    pub category: ValidationCategory,
    pub passed: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_status: Option<i32>,
    pub diagnostics: Option<String>,
    #[serde(default)]
    pub failure_classification: Option<ValidationFailureClassification>,
    #[serde(default)]
    pub fallback_command: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidationFailureClassification {
    Implementation,
    Infrastructure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationCategory {
    Success,
    Formatting,
    Lint,
    Compilation,
    Test,
    Timeout,
    Infrastructure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub steps: Vec<ValidationStepResult>,
}

impl ValidationReport {
    pub fn is_success(&self) -> bool {
        self.steps.iter().all(|s| s.passed)
    }

    pub fn summary(&self) -> String {
        let mut out = String::new();
        for step in &self.steps {
            let status = if step.passed { "PASS" } else { "FAIL" };
            out.push_str(&format!("  {:<40} {}\n", step.command, status));
            let output = step.output();
            if !step.passed && !output.is_empty() {
                out.push_str("    Output:\n");
                for line in output.lines() {
                    out.push_str(&format!("      {}\n", line));
                }
            }
        }
        out
    }

    pub fn is_infrastructure_failure(&self) -> bool {
        self.steps
            .last()
            .is_some_and(ValidationStepResult::is_infrastructure_failure)
    }

    pub fn infrastructure_failure(command: &str, diagnostics: String) -> Self {
        Self {
            steps: vec![ValidationStepResult {
                command: command.to_owned(),
                category: ValidationCategory::Infrastructure,
                passed: false,
                stdout: String::new(),
                stderr: String::new(),
                exit_status: None,
                diagnostics: Some(diagnostics),
                failure_classification: Some(ValidationFailureClassification::Infrastructure),
                fallback_command: None,
            }],
        }
    }
}

impl ValidationStepResult {
    pub fn output(&self) -> String {
        [
            Some(self.stdout.as_str()),
            Some(self.stderr.as_str()),
            self.diagnostics.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim())
        .collect::<Vec<_>>()
        .join("\n")
    }

    pub fn is_infrastructure_failure(&self) -> bool {
        matches!(
            self.category,
            ValidationCategory::Timeout | ValidationCategory::Infrastructure
        )
    }
}

pub trait ValidationRunner: Send + Sync {
    fn run(&self, command: &str, working_dir: &Path) -> Result<ValidationStepResult>;
}

pub struct SystemValidationRunner;

impl ValidationRunner for SystemValidationRunner {
    fn run(&self, command: &str, working_dir: &Path) -> Result<ValidationStepResult> {
        let first = execute(command, working_dir);
        if first.passed {
            return Ok(step(command, first, None, None));
        }
        let classification = classify_failure(&first.output());
        let fallback = (classification == ValidationFailureClassification::Infrastructure)
            .then(|| cargo_offline_command(command))
            .flatten();
        if let Some(fallback_command) = fallback.clone() {
            let retry = execute(&fallback_command, working_dir);
            let retry_classification = (!retry.passed).then(|| classify_failure(&retry.output()));
            let mut result = step(command, retry, Some(fallback_command), retry_classification);
            let initial_output = first.output();
            if !initial_output.is_empty() {
                result.diagnostics = Some(format!("Initial attempt failed:\n{initial_output}"));
            }
            return Ok(result);
        }
        Ok(step(command, first, None, Some(classification)))
    }
}

struct ExecutionResult {
    passed: bool,
    stdout: String,
    stderr: String,
    exit_status: Option<i32>,
    diagnostics: Option<String>,
}

impl ExecutionResult {
    fn output(&self) -> String {
        [
            Some(self.stdout.as_str()),
            Some(self.stderr.as_str()),
            self.diagnostics.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
    }
}

fn execute(command: &str, working_dir: &Path) -> ExecutionResult {
    let mut process = Command::new("sh");
    process.arg("-c").arg(command).current_dir(working_dir);
    let output = match run_command_with_timeout(
        process,
        configured_timeout("ORC_VALIDATION_TIMEOUT_SECS", DEFAULT_VALIDATION_TIMEOUT),
    ) {
        Ok(output) => output,
        Err(error) => {
            return ExecutionResult {
                passed: false,
                stdout: String::new(),
                stderr: String::new(),
                exit_status: None,
                diagnostics: Some(error),
            };
        }
    };
    let passed = output.status.success();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    ExecutionResult {
        passed,
        stdout,
        stderr,
        exit_status: output.status.code(),
        diagnostics: None,
    }
}

fn step(
    command: &str,
    execution: ExecutionResult,
    fallback_command: Option<String>,
    classification: Option<ValidationFailureClassification>,
) -> ValidationStepResult {
    let category = if !execution.passed
        && execution
            .diagnostics
            .as_deref()
            .is_some_and(|diagnostics| diagnostics.contains("timed out"))
    {
        ValidationCategory::Timeout
    } else if !execution.passed
        && execution.diagnostics.is_some()
        && classification == Some(ValidationFailureClassification::Infrastructure)
    {
        ValidationCategory::Infrastructure
    } else {
        classify_validation(
            command,
            &execution.stdout,
            &execution.stderr,
            execution.passed,
        )
    };
    ValidationStepResult {
        command: command.to_string(),
        category,
        passed: execution.passed,
        stdout: execution.stdout,
        stderr: execution.stderr,
        exit_status: execution.exit_status,
        diagnostics: execution.diagnostics,
        failure_classification: classification,
        fallback_command,
    }
}

fn classify_failure(output: &str) -> ValidationFailureClassification {
    let lower = output.to_ascii_lowercase();
    if [
        "could not resolve",
        "failed to download",
        "crates.io",
        "registry",
        "network",
        "dns",
        "timed out",
        "failed to spawn",
        "failed waiting",
        "offline mode",
        "no space left on device",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        ValidationFailureClassification::Infrastructure
    } else {
        ValidationFailureClassification::Implementation
    }
}

fn cargo_offline_command(command: &str) -> Option<String> {
    let (prefix, rest) = command.split_once("cargo")?;
    if !rest.chars().next().is_some_and(char::is_whitespace) || rest.contains("--offline") {
        return None;
    }
    Some(format!("{prefix}cargo --offline{rest}"))
}

fn classify_validation(
    command: &str,
    stdout: &str,
    stderr: &str,
    passed: bool,
) -> ValidationCategory {
    if passed {
        return ValidationCategory::Success;
    }
    let command = command.to_ascii_lowercase();
    let diagnostics = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    if infrastructure_diagnostics(&diagnostics) {
        ValidationCategory::Infrastructure
    } else if command.contains("fmt") || command.contains("format") {
        ValidationCategory::Formatting
    } else if command.contains("clippy") || command.contains("lint") {
        ValidationCategory::Lint
    } else if command.contains("test") || command.contains("pytest") {
        ValidationCategory::Test
    } else {
        ValidationCategory::Compilation
    }
}

fn infrastructure_diagnostics(diagnostics: &str) -> bool {
    [
        "failed to download",
        "failed to fetch",
        "could not resolve host",
        "connection timed out",
        "connection reset",
        "network failure",
        "spurious network error",
        "failed to get successful http response",
        "temporary failure in name resolution",
        "registry index",
    ]
    .iter()
    .any(|pattern| diagnostics.contains(pattern))
}

pub fn run_validation_pipeline(
    runner: &dyn ValidationRunner,
    commands: &[String],
    working_dir: &Path,
) -> Result<ValidationReport> {
    let mut steps = Vec::new();
    for cmd in commands {
        let step = runner.run(cmd, working_dir)?;
        let passed = step.passed;
        steps.push(step);
        if !passed {
            break;
        }
    }
    Ok(ValidationReport { steps })
}

impl ValidationConfig {
    pub fn default_commands() -> Vec<String> {
        vec![
            "cargo fmt --check".to_string(),
            "cargo clippy --all-targets -- -D warnings".to_string(),
            "cargo test".to_string(),
        ]
    }

    pub fn load(repo_path: impl AsRef<Path>) -> Result<Self> {
        let repo_path = repo_path.as_ref();

        // 1. Try .orc/validation.toml
        let toml_path = repo_path.join(".orc/validation.toml");
        if toml_path.exists() {
            let content = std::fs::read_to_string(&toml_path)
                .with_context(|| format!("failed to read {}", toml_path.display()))?;
            if let Some(commands) = parse_commands_from_toml(&content) {
                return Ok(Self {
                    commands,
                    groups: parse_groups_from_toml(&content),
                    boundaries: parse_boundaries_from_toml(&content),
                });
            }
        }

        // 2. Try .orc/validation.json
        let json_path = repo_path.join(".orc/validation.json");
        if json_path.exists() {
            let content = std::fs::read_to_string(&json_path)
                .with_context(|| format!("failed to read {}", json_path.display()))?;
            if let Ok(cfg) = serde_json::from_str::<ValidationConfig>(&content) {
                return Ok(cfg);
            }
        }

        // 3. Try .orc/engineering.md extraction
        let contract_path = repo_path.join(".orc/engineering.md");
        if let Some(commands) = std::fs::read_to_string(&contract_path)
            .ok()
            .and_then(|content| extract_commands_from_engineering_contract(&content))
        {
            return Ok(Self {
                commands,
                groups: Vec::new(),
                boundaries: Vec::new(),
            });
        }

        // 4. Default fallback
        Ok(Self {
            commands: Self::default_commands(),
            groups: Vec::new(),
            boundaries: Vec::new(),
        })
    }
}

fn parse_commands_from_toml(content: &str) -> Option<Vec<String>> {
    let mut in_commands = false;
    let mut commands = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if (trimmed.starts_with("commands") || trimmed.starts_with("commands "))
            && trimmed.contains('=')
        {
            let after_eq = trimmed.split_once('=')?.1.trim();
            if after_eq.starts_with('[') && after_eq.ends_with(']') {
                let inner = &after_eq[1..after_eq.len() - 1];
                for item in inner.split(',') {
                    let s = item.trim().trim_matches('"').trim_matches('\'').trim();
                    if !s.is_empty() {
                        commands.push(s.to_string());
                    }
                }
                return Some(commands);
            }
            if let Some(inner) = after_eq.strip_prefix('[') {
                in_commands = true;
                for item in inner.split(',') {
                    let s = item.trim().trim_matches('"').trim_matches('\'').trim();
                    if !s.is_empty() && s != "]" {
                        commands.push(s.to_string());
                    }
                }
                if after_eq.contains(']') {
                    return Some(commands);
                }
                continue;
            }
        }
        if in_commands {
            if trimmed.ends_with(']') || trimmed == "]" {
                let inner = trimmed.trim_end_matches(']');
                for item in inner.split(',') {
                    let s = item.trim().trim_matches('"').trim_matches('\'').trim();
                    if !s.is_empty() {
                        commands.push(s.to_string());
                    }
                }
                return Some(commands);
            }
            for item in trimmed.split(',') {
                let s = item.trim().trim_matches('"').trim_matches('\'').trim();
                if !s.is_empty() {
                    commands.push(s.to_string());
                }
            }
        }
    }
    if in_commands || !commands.is_empty() {
        Some(commands)
    } else {
        None
    }
}

/// Parse `[[groups]]` array-of-tables from `.orc/validation.toml`. Each group
/// has a `name` and a `commands` array, using the same lightweight bracket
/// parsing as the top-level `commands` key.
fn parse_groups_from_toml(content: &str) -> Vec<ValidationGroup> {
    let mut groups = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_commands: Vec<String> = Vec::new();
    let mut in_group = false;
    let mut in_commands = false;

    fn flush(
        groups: &mut Vec<ValidationGroup>,
        name: &mut Option<String>,
        commands: &mut Vec<String>,
    ) {
        if let Some(name) = name.take() {
            groups.push(ValidationGroup {
                name,
                commands: std::mem::take(commands),
            });
        } else {
            commands.clear();
        }
    }

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "[[groups]]" {
            flush(&mut groups, &mut current_name, &mut current_commands);
            in_group = true;
            in_commands = false;
            continue;
        }
        if trimmed.starts_with('[') {
            flush(&mut groups, &mut current_name, &mut current_commands);
            in_group = false;
            in_commands = false;
            continue;
        }
        if !in_group {
            continue;
        }
        if in_commands {
            if trimmed.ends_with(']') || trimmed == "]" {
                let inner = trimmed.trim_end_matches(']');
                for item in inner.split(',') {
                    let s = item.trim().trim_matches('"').trim_matches('\'').trim();
                    if !s.is_empty() {
                        current_commands.push(s.to_string());
                    }
                }
                in_commands = false;
            } else {
                for item in trimmed.split(',') {
                    let s = item.trim().trim_matches('"').trim_matches('\'').trim();
                    if !s.is_empty() {
                        current_commands.push(s.to_string());
                    }
                }
            }
            continue;
        }
        if trimmed.starts_with("name") && trimmed.contains('=') {
            let Some((_, after_eq)) = trimmed.split_once('=') else {
                continue;
            };
            let value = after_eq.trim().trim_matches('"').trim_matches('\'').trim();
            if !value.is_empty() {
                current_name = Some(value.to_string());
            }
            continue;
        }
        if trimmed.starts_with("commands") && trimmed.contains('=') {
            let Some((_, after_eq)) = trimmed.split_once('=') else {
                continue;
            };
            let after_eq = after_eq.trim();
            if after_eq.starts_with('[') && after_eq.ends_with(']') {
                let inner = &after_eq[1..after_eq.len() - 1];
                for item in inner.split(',') {
                    let s = item.trim().trim_matches('"').trim_matches('\'').trim();
                    if !s.is_empty() {
                        current_commands.push(s.to_string());
                    }
                }
            } else if let Some(inner) = after_eq.strip_prefix('[') {
                in_commands = true;
                for item in inner.split(',') {
                    let s = item.trim().trim_matches('"').trim_matches('\'').trim();
                    if !s.is_empty() && s != "]" {
                        current_commands.push(s.to_string());
                    }
                }
            }
        }
    }
    flush(&mut groups, &mut current_name, &mut current_commands);
    groups
}

/// Parse `[[boundaries]]` array-of-tables from `.orc/validation.toml`. Each
/// boundary has a `path` and a `groups` array, using the same lightweight
/// bracket parsing as `[[groups]]`.
fn parse_boundaries_from_toml(content: &str) -> Vec<ValidationBoundary> {
    let mut boundaries = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_groups: Vec<String> = Vec::new();
    let mut in_boundary = false;
    let mut in_groups = false;

    fn flush(
        boundaries: &mut Vec<ValidationBoundary>,
        path: &mut Option<String>,
        groups: &mut Vec<String>,
    ) {
        if let Some(path) = path.take() {
            boundaries.push(ValidationBoundary {
                path,
                groups: std::mem::take(groups),
            });
        } else {
            groups.clear();
        }
    }

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "[[boundaries]]" {
            flush(&mut boundaries, &mut current_path, &mut current_groups);
            in_boundary = true;
            in_groups = false;
            continue;
        }
        if trimmed.starts_with('[') {
            flush(&mut boundaries, &mut current_path, &mut current_groups);
            in_boundary = false;
            in_groups = false;
            continue;
        }
        if !in_boundary {
            continue;
        }
        if in_groups {
            if trimmed.ends_with(']') || trimmed == "]" {
                let inner = trimmed.trim_end_matches(']');
                for item in inner.split(',') {
                    let s = item.trim().trim_matches('"').trim_matches('\'').trim();
                    if !s.is_empty() {
                        current_groups.push(s.to_string());
                    }
                }
                in_groups = false;
            } else {
                for item in trimmed.split(',') {
                    let s = item.trim().trim_matches('"').trim_matches('\'').trim();
                    if !s.is_empty() {
                        current_groups.push(s.to_string());
                    }
                }
            }
            continue;
        }
        if trimmed.starts_with("path") && trimmed.contains('=') {
            let Some((_, after_eq)) = trimmed.split_once('=') else {
                continue;
            };
            let value = after_eq.trim().trim_matches('"').trim_matches('\'').trim();
            if !value.is_empty() {
                current_path = Some(value.to_string());
            }
            continue;
        }
        if trimmed.starts_with("groups") && trimmed.contains('=') {
            let Some((_, after_eq)) = trimmed.split_once('=') else {
                continue;
            };
            let after_eq = after_eq.trim();
            if after_eq.starts_with('[') && after_eq.ends_with(']') {
                let inner = &after_eq[1..after_eq.len() - 1];
                for item in inner.split(',') {
                    let s = item.trim().trim_matches('"').trim_matches('\'').trim();
                    if !s.is_empty() {
                        current_groups.push(s.to_string());
                    }
                }
            } else if let Some(inner) = after_eq.strip_prefix('[') {
                in_groups = true;
                for item in inner.split(',') {
                    let s = item.trim().trim_matches('"').trim_matches('\'').trim();
                    if !s.is_empty() && s != "]" {
                        current_groups.push(s.to_string());
                    }
                }
            }
        }
    }
    flush(&mut boundaries, &mut current_path, &mut current_groups);
    boundaries
}

fn extract_commands_from_engineering_contract(content: &str) -> Option<Vec<String>> {
    let mut capturing = false;
    let mut commands = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed
            .to_lowercase()
            .contains("every implementation must pass:")
            || trimmed
                .to_lowercase()
                .starts_with("## tests and validation")
        {
            capturing = true;
            continue;
        }
        if capturing {
            if trimmed.starts_with('#') {
                break;
            }
            if trimmed.starts_with("cargo ")
                || trimmed.starts_with("npm ")
                || trimmed.starts_with("pytest ")
                || trimmed.starts_with("make ")
            {
                commands.push(trimmed.to_string());
            } else if !commands.is_empty() && trimmed.is_empty() {
                break;
            }
        }
    }
    if !commands.is_empty() {
        Some(commands)
    } else {
        None
    }
}

pub mod test_helpers {
    use super::*;
    use std::sync::Mutex;

    pub struct FakeValidationRunner {
        pub fail_commands: Vec<String>,
        pub executed: Mutex<Vec<String>>,
    }

    impl FakeValidationRunner {
        pub fn success() -> Self {
            Self {
                fail_commands: Vec::new(),
                executed: Mutex::new(Vec::new()),
            }
        }

        pub fn failing_on(command: &str) -> Self {
            Self {
                fail_commands: vec![command.to_string()],
                executed: Mutex::new(Vec::new()),
            }
        }

        pub fn executed_commands(&self) -> Vec<String> {
            self.executed.lock().unwrap().clone()
        }
    }

    impl ValidationRunner for FakeValidationRunner {
        fn run(&self, command: &str, _working_dir: &Path) -> Result<ValidationStepResult> {
            self.executed.lock().unwrap().push(command.to_string());
            let passed = !self.fail_commands.iter().any(|c| c == command);
            let output = if passed {
                String::new()
            } else {
                format!("command failed: {}", command)
            };
            Ok(ValidationStepResult {
                command: command.to_string(),
                category: if passed {
                    ValidationCategory::Success
                } else {
                    classify_validation(command, "", &output, false)
                },
                passed,
                stdout: String::new(),
                stderr: output,
                exit_status: Some(if passed { 0 } else { 1 }),
                diagnostics: None,
                failure_classification: (!passed)
                    .then_some(ValidationFailureClassification::Implementation),
                fallback_command: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::FakeValidationRunner;
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parse_toml_single_line_and_multiline() {
        let toml_single = r#"commands = ["cargo fmt --check", "cargo test"]"#;
        let cmds = parse_commands_from_toml(toml_single).unwrap();
        assert_eq!(cmds, vec!["cargo fmt --check", "cargo test"]);

        let toml_multi = r#"
commands = [
  "cargo fmt --check",
  "cargo clippy --all-targets -- -D warnings",
  "cargo test",
]
"#;
        let cmds2 = parse_commands_from_toml(toml_multi).unwrap();
        assert_eq!(
            cmds2,
            vec![
                "cargo fmt --check",
                "cargo clippy --all-targets -- -D warnings",
                "cargo test"
            ]
        );
    }

    #[test]
    fn load_from_toml_and_json_and_contract() {
        let dir = tempdir().unwrap();
        let orc_dir = dir.path().join(".orc");
        std::fs::create_dir_all(&orc_dir).unwrap();

        // 1. Engineering contract
        std::fs::write(
            orc_dir.join("engineering.md"),
            "# Contract\n\n## Tests and validation\nEvery implementation must pass:\n\ncargo test\n",
        )
        .unwrap();
        let cfg = ValidationConfig::load(dir.path()).unwrap();
        assert_eq!(cfg.commands, vec!["cargo test"]);

        // 2. validation.json overrides contract
        std::fs::write(
            orc_dir.join("validation.json"),
            r#"{"commands": ["cargo check"]}"#,
        )
        .unwrap();
        let cfg = ValidationConfig::load(dir.path()).unwrap();
        assert_eq!(cfg.commands, vec!["cargo check"]);

        // 3. validation.toml overrides json
        std::fs::write(
            orc_dir.join("validation.toml"),
            r#"commands = ["cargo test --lib"]"#,
        )
        .unwrap();
        let cfg = ValidationConfig::load(dir.path()).unwrap();
        assert_eq!(cfg.commands, vec!["cargo test --lib"]);
    }

    #[test]
    fn this_repo_validation_toml_declares_the_tauri_frontend_boundary() {
        // Regression guard tying the parser to the real .orc/validation.toml:
        // the Tauri command surface and its hand-mirrored frontend contract
        // must stay declared as a boundary, or a change to either would be
        // under-validated as a single-subsystem change.
        let cfg = ValidationConfig::load(Path::new(".")).unwrap();
        let lib_rs = cfg
            .boundaries
            .iter()
            .find(|boundary| boundary.path == "src-tauri/src/lib.rs")
            .expect("src-tauri/src/lib.rs boundary is configured");
        assert!(lib_rs.groups.contains(&"tauri".to_string()));
        assert!(lib_rs.groups.contains(&"frontend".to_string()));
        for group in &lib_rs.groups {
            assert!(
                cfg.groups.iter().any(|g| &g.name == group),
                "boundary references unconfigured group '{group}'"
            );
        }
    }

    #[test]
    fn parse_groups_from_toml_reads_named_command_sets() {
        let toml = r#"
commands = ["cargo fmt --check"]

[[groups]]
name = "rust-core"
commands = ["cargo fmt --check", "cargo test"]

[[groups]]
name = "frontend"
commands = [
  "npm run typecheck",
  "npm run build",
]
"#;
        let groups = parse_groups_from_toml(toml);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "rust-core");
        assert_eq!(groups[0].commands, vec!["cargo fmt --check", "cargo test"]);
        assert_eq!(groups[1].name, "frontend");
        assert_eq!(
            groups[1].commands,
            vec!["npm run typecheck", "npm run build"]
        );
    }

    #[test]
    fn parse_boundaries_from_toml_reads_multi_group_paths() {
        let toml = r#"
commands = ["cargo fmt --check"]

[[groups]]
name = "rust-core"
commands = ["cargo test"]

[[boundaries]]
path = "src-tauri/src/lib.rs"
groups = ["tauri", "frontend"]

[[boundaries]]
path = "src/shared/schema.rs"
groups = [
  "rust-core",
  "frontend",
]
"#;
        let boundaries = parse_boundaries_from_toml(toml);
        assert_eq!(boundaries.len(), 2);
        assert_eq!(boundaries[0].path, "src-tauri/src/lib.rs");
        assert_eq!(boundaries[0].groups, vec!["tauri", "frontend"]);
        assert_eq!(boundaries[1].path, "src/shared/schema.rs");
        assert_eq!(boundaries[1].groups, vec!["rust-core", "frontend"]);
    }

    fn grouped_config() -> ValidationConfig {
        ValidationConfig {
            commands: vec!["cargo fmt --check".into()],
            groups: vec![
                ValidationGroup {
                    name: "rust-core".into(),
                    commands: vec!["cargo fmt --check".into(), "cargo test".into()],
                },
                ValidationGroup {
                    name: "frontend".into(),
                    commands: vec!["npm run typecheck".into(), "npm run build".into()],
                },
                ValidationGroup {
                    name: "tauri".into(),
                    commands: vec!["cargo test --manifest-path src-tauri/Cargo.toml".into()],
                },
                ValidationGroup {
                    name: "packaging".into(),
                    commands: vec!["npm run validate:package".into()],
                },
                ValidationGroup {
                    name: "integration".into(),
                    commands: vec![
                        "cargo fmt --check".into(),
                        "cargo test".into(),
                        "npm run typecheck".into(),
                        "npm run build".into(),
                        "cargo test --manifest-path src-tauri/Cargo.toml".into(),
                    ],
                },
            ],
            boundaries: vec![ValidationBoundary {
                path: "src-tauri/src/lib.rs".into(),
                groups: vec!["tauri".into(), "frontend".into()],
            }],
        }
    }

    #[test]
    fn rust_only_task_selects_only_rust_core_group() {
        let config = grouped_config();
        let selection =
            config.select_for_task(&["src/agent.rs".into(), "src/storage/db.rs".into()], &[]);
        assert_eq!(selection.groups, vec!["rust-core"]);
        assert_eq!(selection.commands, vec!["cargo fmt --check", "cargo test"]);
    }

    #[test]
    fn frontend_only_task_selects_only_frontend_group() {
        let config = grouped_config();
        let selection = config.select_for_task(&["src/App.vue".into(), "package.json".into()], &[]);
        assert_eq!(selection.groups, vec!["frontend"]);
        assert_eq!(
            selection.commands,
            vec!["npm run typecheck", "npm run build"]
        );
        assert!(
            !selection
                .commands
                .iter()
                .any(|command| command.starts_with("cargo"))
        );
    }

    #[test]
    fn cross_cutting_task_selects_multiple_relevant_groups() {
        let config = grouped_config();
        let selection =
            config.select_for_task(&["src-tauri/src/lib.rs".into(), "src/App.vue".into()], &[]);
        assert_eq!(selection.groups, vec!["frontend", "tauri"]);
        assert!(
            selection
                .commands
                .contains(&"npm run typecheck".to_string())
        );
        assert!(
            selection
                .commands
                .contains(&"cargo test --manifest-path src-tauri/Cargo.toml".to_string())
        );
    }

    #[test]
    fn tauri_command_boundary_file_alone_selects_tauri_and_frontend_not_full_integration() {
        // src-tauri/src/lib.rs is the Tauri command surface consumed by the
        // frontend; a path heuristic alone (starts_with("src-tauri/")) would
        // under-select just "tauri" and miss that the frontend depends on
        // this exact contract. The explicit [[boundaries]] entry fixes that
        // without escalating to the full "integration" group.
        let config = grouped_config();
        let selection = config.select_for_task(&["src-tauri/src/lib.rs".into()], &[]);
        assert_eq!(selection.groups, vec!["frontend", "tauri"]);
        assert!(
            selection
                .commands
                .contains(&"npm run typecheck".to_string())
        );
        assert!(
            selection
                .commands
                .contains(&"cargo test --manifest-path src-tauri/Cargo.toml".to_string())
        );
        assert!(!selection.groups.contains(&"integration".to_string()));
        assert!(!selection.groups.contains(&"packaging".to_string()));
    }

    #[test]
    fn boundary_file_alongside_unrelated_rust_change_unions_exactly_the_affected_groups() {
        // A shared-contract change plus an ordinary rust-core change must
        // select the union of directly affected groups, not escalate to the
        // full pipeline merely because more than one group is involved.
        let config = grouped_config();
        let selection = config.select_for_task(
            &["src-tauri/src/lib.rs".into(), "src/storage/db.rs".into()],
            &[],
        );
        assert_eq!(selection.groups, vec!["frontend", "rust-core", "tauri"]);
        assert!(!selection.groups.contains(&"integration".to_string()));
    }

    #[test]
    fn packaging_only_change_selects_only_packaging_validation() {
        let config = grouped_config();
        let selection = config.select_for_task(&["packaging/orc.nsi".into()], &[]);
        assert_eq!(selection.groups, vec!["packaging"]);
        assert_eq!(
            selection.commands,
            vec!["npm run validate:package".to_string()]
        );
        assert!(!selection.commands.iter().any(
            |command| command.starts_with("cargo") || command.starts_with("npm run typecheck")
        ));
    }

    #[test]
    fn explicit_task_required_command_is_included_when_known() {
        let config = grouped_config();
        let selection = config.select_for_task(
            &["src/agent.rs".into()],
            &["npm run build".into(), "made up command".into()],
        );
        assert!(selection.commands.contains(&"npm run build".to_string()));
        assert!(!selection.commands.contains(&"made up command".to_string()));
    }

    #[test]
    fn unclassified_changed_files_escalate_to_integration_group_not_every_command_by_default() {
        let config = grouped_config();
        let selection = config.select_for_task(&["README.md".into()], &[]);
        assert_eq!(selection.groups, vec!["integration"]);
        assert_eq!(
            selection.commands,
            config.group("integration").unwrap().commands
        );
    }

    #[test]
    fn selected_commands_are_deduplicated() {
        let config = grouped_config();
        let selection = config.select_for_task(&["src/agent.rs".into()], &["cargo test".into()]);
        let occurrences = selection
            .commands
            .iter()
            .filter(|command| *command == "cargo test")
            .count();
        assert_eq!(occurrences, 1);
    }

    #[test]
    fn fake_runner_execution_and_pipeline() {
        let runner = FakeValidationRunner::failing_on("cargo test");
        let commands = vec!["cargo fmt --check".to_string(), "cargo test".to_string()];
        let report = run_validation_pipeline(&runner, &commands, Path::new(".")).unwrap();
        assert!(!report.is_success());
        assert_eq!(report.steps.len(), 2);
        assert!(report.steps[0].passed);
        assert!(!report.steps[1].passed);
        assert_eq!(runner.executed_commands(), commands);
    }

    #[test]
    fn cargo_offline_fallback_preserves_manifest_arguments() {
        assert_eq!(
            cargo_offline_command("cargo test --manifest-path crates/app/Cargo.toml --lib"),
            Some("cargo --offline test --manifest-path crates/app/Cargo.toml --lib".into())
        );
        assert_eq!(cargo_offline_command("cargo test --offline"), None);
    }

    #[test]
    fn failures_are_classified_without_fallback_for_implementation_errors() {
        assert_eq!(
            classify_failure("test foo failed: assertion failed"),
            ValidationFailureClassification::Implementation
        );
        assert_eq!(
            classify_failure("failed to download from crates.io: network error"),
            ValidationFailureClassification::Infrastructure
        );
    }

    #[test]
    fn system_runner_preserves_streams_status_and_classifies_failures() {
        let runner = SystemValidationRunner;
        let result = runner
            .run(
                "printf 'standard output'; printf 'format error' >&2; exit 7 # fmt",
                Path::new("."),
            )
            .unwrap();
        assert_eq!(result.category, ValidationCategory::Formatting);
        assert_eq!(result.stdout, "standard output");
        assert_eq!(result.stderr, "format error");
        assert_eq!(result.exit_status, Some(7));

        let result = runner
            .run(
                "printf 'spurious network error: registry unavailable' >&2; exit 1",
                Path::new("."),
            )
            .unwrap();
        assert_eq!(result.category, ValidationCategory::Infrastructure);
    }
}
