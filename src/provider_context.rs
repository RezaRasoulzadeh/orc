//! Provider-independent accounting for the context surrounding one invocation.
//!
//! Orc can measure the packet and instruction files it owns. Provider bootstrap
//! prompts, tool schemas, and any provider-side cache composition are explicitly
//! represented as unknown rather than inferred from byte counts.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::execution_packet::{PacketMetadata, Truncation};

pub const CONTEXT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextMeasurement {
    DirectlyMeasured,
    DeterministicallyCalculated,
    ProviderReported,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSessionState {
    New,
    Reused,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketSectionSize {
    pub name: String,
    pub bytes: usize,
    pub characters: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketContext {
    pub packet_type: String,
    /// Exact UTF-8 size of the pretty JSON packet, excluding fixed role text.
    pub bytes: usize,
    pub characters: usize,
    pub rendered_prompt_bytes: usize,
    pub rendered_prompt_characters: usize,
    pub sections: Vec<PacketSectionSize>,
    pub truncated: bool,
    pub truncations: Vec<Truncation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSource {
    pub category: String,
    pub identifier: String,
    pub bytes: Option<usize>,
    pub characters: Option<usize>,
    pub measurement: ContextMeasurement,
    /// Whether this source was intentionally included in provider context.
    /// Excluded profile configuration is still measured to make the reduction
    /// auditable without claiming it was sent.
    pub included: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInvocationContext {
    pub schema_version: u32,
    pub action: String,
    pub provider: String,
    pub packet: PacketContext,
    pub context_sources: Vec<ContextSource>,
    pub execution_environment: String,
    pub repository_filesystem_access: bool,
    pub repository_context_discovery: bool,
    pub session_state: ProviderSessionState,
    pub provider_context_breakdown_available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedPacket {
    pub content: String,
    pub packet: PacketContext,
    pub fixed_instructions: ContextSource,
}

#[derive(Clone, Copy, Debug)]
pub struct InvocationEnvironment<'a> {
    pub profile_path: Option<&'a Path>,
    pub cwd: &'a Path,
    pub repository_context_discovery: bool,
    pub isolated: bool,
    pub repository_filesystem_access: bool,
}

/// Render once for transport and calculate stable top-level section sizes from
/// the same value. Section sizes use compact JSON values and intentionally do
/// not claim to sum to the pretty packet size (keys and JSON punctuation are
/// part of the latter).
pub fn render_accounted<T: Serialize>(
    role_instructions: &str,
    packet_metadata: &PacketMetadata,
    packet: &T,
) -> Result<RenderedPacket> {
    let value = serde_json::to_value(packet)?;
    let packet_json = serde_json::to_string_pretty(&value)?;
    let mut sections = value
        .as_object()
        .into_iter()
        .flat_map(|object| object.iter())
        .map(|(name, value)| {
            let rendered = serde_json::to_string(value).expect("JSON value serialization");
            PacketSectionSize {
                name: name.clone(),
                bytes: rendered.len(),
                characters: rendered.chars().count(),
            }
        })
        .collect::<Vec<_>>();
    sections.sort_by(|left, right| left.name.cmp(&right.name));
    let content = format!("{role_instructions}\n\n## Authoritative Orc packet\n\n{packet_json}");
    Ok(RenderedPacket {
        packet: PacketContext {
            packet_type: packet_metadata.packet_type.clone(),
            bytes: packet_json.len(),
            characters: packet_json.chars().count(),
            rendered_prompt_bytes: content.len(),
            rendered_prompt_characters: content.chars().count(),
            sections,
            truncated: !packet_metadata.truncations.is_empty(),
            truncations: packet_metadata.truncations.clone(),
        },
        fixed_instructions: measured_source(
            "orc_fixed_instructions",
            "fixed action instructions",
            role_instructions,
            ContextMeasurement::DirectlyMeasured,
        ),
        content,
    })
}

/// Account for a bounded Orc-generated prompt that is not represented by a
/// typed packet (currently the worker completion self-check repair).
pub fn account_unstructured(packet_type: &str, prompt: String) -> RenderedPacket {
    let section = PacketSectionSize {
        name: "repair_context".into(),
        bytes: prompt.len(),
        characters: prompt.chars().count(),
    };
    RenderedPacket {
        packet: PacketContext {
            packet_type: packet_type.into(),
            bytes: prompt.len(),
            characters: prompt.chars().count(),
            rendered_prompt_bytes: prompt.len(),
            rendered_prompt_characters: prompt.chars().count(),
            sections: vec![section],
            truncated: false,
            truncations: Vec::new(),
        },
        fixed_instructions: ContextSource {
            category: "orc_fixed_instructions".into(),
            identifier: "embedded completion-repair instructions".into(),
            bytes: Some(0),
            characters: Some(0),
            measurement: ContextMeasurement::DirectlyMeasured,
            included: true,
        },
        content: prompt,
    }
}

pub fn invocation_context(
    action: &str,
    provider: &str,
    rendered: &RenderedPacket,
    environment: InvocationEnvironment<'_>,
) -> ProviderInvocationContext {
    let cwd = environment
        .cwd
        .canonicalize()
        .unwrap_or_else(|_| environment.cwd.to_path_buf());
    let cwd = cwd.as_path();
    let mut sources = vec![rendered.fixed_instructions.clone()];
    if let Some(profile) = environment.profile_path {
        sources.extend(known_profile_sources(profile, provider == "codex"));
    }
    if environment.repository_context_discovery {
        sources.extend(repository_instruction_sources(cwd));
    } else if provider == "codex" && environment.repository_filesystem_access {
        sources.extend(
            repository_instruction_sources(cwd)
                .into_iter()
                .map(|mut source| {
                    source.category = "excluded_repository_instructions".into();
                    source.included = false;
                    source
                }),
        );
    }
    sources.push(ContextSource {
        category: "provider_runtime_context".into(),
        identifier: format!("{provider} bootstrap, tool schemas, and integration context"),
        bytes: None,
        characters: None,
        measurement: ContextMeasurement::Unknown,
        included: true,
    });
    sources.sort_by(|left, right| {
        left.category
            .cmp(&right.category)
            .then(left.identifier.cmp(&right.identifier))
    });
    ProviderInvocationContext {
        schema_version: CONTEXT_SCHEMA_VERSION,
        action: action.into(),
        provider: provider.into(),
        packet: rendered.packet.clone(),
        context_sources: sources,
        execution_environment: if environment.isolated && environment.repository_filesystem_access {
            "isolated_with_explicit_repository_access".into()
        } else if environment.isolated {
            "isolated_non_repository".into()
        } else if environment.repository_context_discovery {
            "repository_worktree".into()
        } else {
            "caller_working_directory".into()
        },
        repository_filesystem_access: environment.repository_filesystem_access,
        repository_context_discovery: environment.repository_context_discovery,
        // Every existing Orc command transport starts a new process and never
        // passes `codex exec resume` or another provider session identifier.
        session_state: if provider == "codex" {
            ProviderSessionState::New
        } else {
            ProviderSessionState::Unknown
        },
        provider_context_breakdown_available: false,
    }
}

pub fn add_known_text_source(
    context: &mut ProviderInvocationContext,
    category: &str,
    identifier: &str,
    value: &str,
) {
    context.context_sources.push(measured_source(
        category,
        identifier,
        value,
        ContextMeasurement::DeterministicallyCalculated,
    ));
    context.context_sources.sort_by(|left, right| {
        left.category
            .cmp(&right.category)
            .then(left.identifier.cmp(&right.identifier))
    });
}

fn measured_source(
    category: &str,
    identifier: &str,
    value: &str,
    measurement: ContextMeasurement,
) -> ContextSource {
    ContextSource {
        category: category.into(),
        identifier: identifier.into(),
        bytes: Some(value.len()),
        characters: Some(value.chars().count()),
        measurement,
        included: true,
    }
}

fn file_source(category: &str, path: &Path) -> Option<ContextSource> {
    let value = std::fs::read_to_string(path).ok()?;
    Some(measured_source(
        category,
        &path.display().to_string(),
        &value,
        ContextMeasurement::DeterministicallyCalculated,
    ))
}

fn known_profile_sources(profile: &Path, codex_ignores_user_config: bool) -> Vec<ContextSource> {
    ["AGENTS.override.md", "AGENTS.md", "config.toml"]
        .into_iter()
        .filter_map(|name| {
            let category = if name == "config.toml" {
                "agent_profile_configuration"
            } else {
                "agent_profile_instructions"
            };
            let mut source = file_source(category, &profile.join(name))?;
            if name == "config.toml" && codex_ignores_user_config {
                source.category = "excluded_agent_profile_configuration".into();
                source.included = false;
            }
            Some(source)
        })
        .collect()
}

fn repository_instruction_sources(cwd: &Path) -> Vec<ContextSource> {
    let ancestors = cwd.ancestors().map(PathBuf::from).collect::<Vec<_>>();
    let root_index = ancestors
        .iter()
        .position(|directory| directory.join(".git").exists())
        .unwrap_or_else(|| ancestors.len().saturating_sub(1));
    let mut directories = ancestors
        .into_iter()
        .take(root_index + 1)
        .collect::<Vec<_>>();
    directories.reverse();
    let mut sources = Vec::new();
    for directory in directories {
        for name in ["AGENTS.override.md", "AGENTS.md"] {
            if let Some(source) =
                file_source("repository_discovered_instructions", &directory.join(name))
            {
                sources.push(source);
            }
        }
    }
    sources
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_accounting_is_exact_ordered_and_does_not_store_raw_sources() {
        let packet = serde_json::json!({"zeta": "é", "alpha": [1, 2, 3]});
        let metadata = PacketMetadata {
            packet_type: "dispatch".into(),
            truncations: vec![Truncation {
                field: "zeta".into(),
                omitted_items: 0,
                omitted_bytes: 10,
            }],
            ..Default::default()
        };
        let rendered = render_accounted("fixed", &metadata, &packet).unwrap();
        let json = rendered
            .content
            .split_once("## Authoritative Orc packet\n\n")
            .unwrap()
            .1;
        assert_eq!(rendered.packet.bytes, json.len());
        assert_eq!(rendered.packet.characters, json.chars().count());
        assert_eq!(
            rendered
                .packet
                .sections
                .iter()
                .map(|section| section.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "zeta"]
        );
        assert!(rendered.packet.truncated);
        assert_eq!(rendered.packet.truncations[0].field, "zeta");
    }

    #[test]
    fn known_and_unknown_context_sources_are_distinct() {
        let profile = tempfile::tempdir().unwrap();
        std::fs::write(profile.path().join("AGENTS.md"), "profile rules").unwrap();
        std::fs::write(profile.path().join("config.toml"), "model = 'test'").unwrap();
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("AGENTS.md"), "repository rules").unwrap();
        let metadata = PacketMetadata {
            packet_type: "dispatch".into(),
            ..Default::default()
        };
        let rendered = render_accounted("fixed", &metadata, &serde_json::json!({"x": 1})).unwrap();
        let context = invocation_context(
            "code",
            "codex",
            &rendered,
            InvocationEnvironment {
                profile_path: Some(profile.path()),
                cwd: repo.path(),
                repository_context_discovery: true,
                isolated: false,
                repository_filesystem_access: true,
            },
        );
        assert_eq!(context.session_state, ProviderSessionState::New);
        assert!(context.repository_context_discovery);
        assert!(context.context_sources.iter().any(|source| {
            source.category == "repository_discovered_instructions"
                && source.bytes == Some("repository rules".len())
        }));
        assert!(context.context_sources.iter().any(|source| {
            source.category == "agent_profile_instructions"
                && source.bytes == Some("profile rules".len())
        }));
        assert!(context.context_sources.iter().any(|source| {
            source.category == "excluded_agent_profile_configuration"
                && !source.included
                && source.bytes == Some("model = 'test'".len())
        }));
        assert!(context.context_sources.iter().any(|source| {
            source.category == "provider_runtime_context"
                && source.measurement == ContextMeasurement::Unknown
                && source.bytes.is_none()
        }));
    }
}
