use crate::codex_app_server::{self, CodexAppServer};
use crate::registry::{self, AgentDefinition};
use crate::storage::Database;
use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum AgentCommand {
    List,
    Add {
        id: String,
        #[arg(long)]
        backend: String,
        #[arg(long, default_value_t = 0)]
        priority: i64,
        #[arg(long)]
        capability: Vec<String>,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_parser = parse_reasoning_effort)]
        effort: Option<registry::ReasoningEffort>,
        #[arg(long, value_parser = ["automated", "manual"], default_value = "automated")]
        mode: String,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    Remove {
        id: String,
    },
    Unavailable {
        id: String,
        reason: String,
    },
    Available {
        id: String,
    },
    Priority {
        id: String,
        priority: i64,
    },
    /// Set the configuration profile directory for an existing agent.
    Profile {
        id: String,
        path: String,
    },
    Model {
        id: String,
        model: String,
    },
    Effort {
        id: String,
        #[arg(value_parser = parse_reasoning_effort)]
        effort: registry::ReasoningEffort,
    },
    Quota {
        id: String,
        #[arg(long, value_parser = clap::value_parser!(i64).range(0..=100))]
        remaining: i64,
        #[arg(long)]
        reset: Option<String>,
    },
    QuotaClear {
        id: String,
    },
    QuotaReserve {
        #[arg(value_parser = clap::value_parser!(i64).range(0..=100))]
        remaining: i64,
    },
    /// Synchronize quota through the provider's machine-readable protocol.
    Sync {
        id: String,
    },
    Show {
        id: String,
    },
}

pub fn run(command: AgentCommand, db_path: &str) -> Result<()> {
    let db = Database::open(db_path).map_err(|e| anyhow::anyhow!(e))?;
    match command {
        AgentCommand::List => {
            print_agents(&db)?;
        }
        AgentCommand::Add {
            id,
            backend,
            priority,
            capability,
            display_name,
            profile,
            model,
            effort,
            mode,
        } => {
            registry::validate_backend(&backend)?;
            if (model.is_some() || effort.is_some())
                && (backend != "codex" || mode == registry::MANUAL)
            {
                anyhow::bail!(
                    "only automated Codex agents support model and reasoning-effort configuration"
                );
            }
            if mode == registry::AUTOMATED
                && backend != "codex"
                && backend != "copilot"
                && backend != "antigravity"
            {
                anyhow::bail!("backend '{}' requires --mode manual", backend);
            }
            let agent = AgentDefinition {
                display_name: display_name.unwrap_or_else(|| id.clone()),
                id,
                backend,
                execution_mode: mode,
                enabled: true,
                priority,
                capabilities: capability,
                status: registry::AVAILABLE.to_owned(),
                unavailable_reason: None,
                profile_path: profile,
                model,
                reasoning_effort: effort,
                config_metadata: None,
                quota_remaining_percent: None,
                quota_reset_at: None,
                quota_checked_at: None,
                quota_source: None,
                quota_limits: None,
            };
            db.insert_agent(&agent).map_err(|e| anyhow::anyhow!(e))?;
            println!("Added agent {}", agent.id);
        }
        AgentCommand::Enable { id } => update_agent_enabled(&db, &id, true)?,
        AgentCommand::Disable { id } => update_agent_enabled(&db, &id, false)?,
        AgentCommand::Remove { id } => {
            db.archive_agent(&id).map_err(|e| anyhow::anyhow!(e))?;
            println!("Archived agent {}", id);
        }
        AgentCommand::Unavailable { id, reason } => {
            ensure_agent_updated(
                db.set_agent_availability(&id, registry::UNAVAILABLE, Some(&reason))
                    .map_err(|e| anyhow::anyhow!(e))?,
                &id,
            )?;
        }
        AgentCommand::Available { id } => {
            ensure_agent_updated(
                db.set_agent_availability(&id, registry::AVAILABLE, None)
                    .map_err(|e| anyhow::anyhow!(e))?,
                &id,
            )?;
        }
        AgentCommand::Priority { id, priority } => ensure_agent_updated(
            db.set_agent_priority(&id, priority)
                .map_err(|e| anyhow::anyhow!(e))?,
            &id,
        )?,
        AgentCommand::Profile { id, path } => ensure_agent_updated(
            db.set_agent_profile_path(&id, &path)
                .map_err(|e| anyhow::anyhow!(e))?,
            &id,
        )?,
        AgentCommand::Model { id, model } => {
            ensure_codex_automated_agent(&db, &id)?;
            ensure_agent_updated(
                db.set_agent_model(&id, &model)
                    .map_err(|e| anyhow::anyhow!(e))?,
                &id,
            )?;
        }
        AgentCommand::Effort { id, effort } => {
            ensure_codex_automated_agent(&db, &id)?;
            ensure_agent_updated(
                db.set_agent_reasoning_effort(&id, effort)
                    .map_err(|e| anyhow::anyhow!(e))?,
                &id,
            )?;
        }
        AgentCommand::Quota {
            id,
            remaining,
            reset,
        } => ensure_agent_updated(
            db.set_agent_quota(&id, remaining, reset.as_deref())
                .map_err(|e| anyhow::anyhow!(e))?,
            &id,
        )?,
        AgentCommand::QuotaClear { id } => ensure_agent_updated(
            db.clear_agent_quota(&id).map_err(|e| anyhow::anyhow!(e))?,
            &id,
        )?,
        AgentCommand::QuotaReserve { remaining } => {
            db.set_quota_reserve(remaining)
                .map_err(|e| anyhow::anyhow!(e))?;
            println!("Automatic dispatch quota reserve set to {remaining}%.");
        }
        AgentCommand::Sync { id } => {
            let agent = registry::get_agent(&db, &id)?;
            let snapshot = codex_app_server::sync_agent(&db, &agent, &CodexAppServer)
                .map_err(anyhow::Error::msg)?;
            print_synced_quota(&id, &snapshot);
        }
        AgentCommand::Show { id } => {
            let agent = registry::get_agent(&db, &id)?;
            println!("ID:                 {}", agent.id);
            println!("Backend:            {}", agent.backend);
            println!("Execution mode:     {}", agent.execution_mode);
            println!(
                "Model:              {}",
                agent.model.as_deref().unwrap_or("-")
            );
            println!(
                "Reasoning effort:   {}",
                agent
                    .reasoning_effort
                    .map(|value| value.as_str())
                    .unwrap_or("-")
            );
            println!("Display name:       {}", agent.display_name);
            println!("Enabled:            {}", agent.enabled);
            println!("Availability:       {}", agent.status);
            println!(
                "Unavailable reason: {}",
                agent.unavailable_reason.as_deref().unwrap_or("-")
            );
            println!("Priority:            {}", agent.priority);
            println!(
                "Quota:               {}",
                agent
                    .quota_remaining_percent
                    .map(|value| format!("{value}%"))
                    .unwrap_or_else(|| "unknown".into())
            );
            println!(
                "Quota reset:         {}",
                agent
                    .quota_reset_at
                    .as_deref()
                    .map(format_timestamp)
                    .unwrap_or_else(|| "-".into())
            );
            println!(
                "Quota checked:       {}",
                agent
                    .quota_checked_at
                    .as_deref()
                    .map(format_timestamp)
                    .unwrap_or_else(|| "-".into())
            );
            println!(
                "Quota source:        {}",
                agent.quota_source.as_deref().unwrap_or("-")
            );
            if let Some(limits) = &agent.quota_limits {
                println!("Effective limit:    {}", limits.effective);
                print_quota_limit("Primary limit:", limits.primary.as_ref());
                print_quota_limit("Secondary limit:", limits.secondary.as_ref());
                if let Some(limit) = &limits.individual_limit {
                    println!(
                        "Individual limit:   {}% remaining, reset {}",
                        limit.remaining_percent,
                        format_timestamp(&limit.reset_at.to_string())
                    );
                }
            }
            println!("Capabilities:        {}", agent.capabilities.join(", "));
            println!(
                "Profile/config:      {}",
                agent.profile_path.as_deref().unwrap_or("-")
            );
        }
    }

    Ok(())
}

fn parse_reasoning_effort(value: &str) -> Result<registry::ReasoningEffort, String> {
    registry::ReasoningEffort::parse(value).map_err(|e| e.to_string())
}
fn ensure_agent_updated(changed: bool, id: &str) -> Result<()> {
    if !changed {
        anyhow::bail!("agent '{}' is not registered", id);
    }
    Ok(())
}
fn update_agent_enabled(db: &Database, id: &str, enabled: bool) -> Result<()> {
    ensure_agent_updated(
        db.set_agent_enabled(id, enabled)
            .map_err(|e| anyhow::anyhow!(e))?,
        id,
    )
}
fn ensure_codex_automated_agent(db: &Database, id: &str) -> Result<()> {
    let agent = registry::get_agent(db, id)?;
    if agent.backend != "codex" || agent.execution_mode != registry::AUTOMATED {
        anyhow::bail!(
            "only automated Codex agents support model and reasoning-effort configuration"
        );
    }
    Ok(())
}
fn format_timestamp(value: &str) -> String {
    value.to_owned()
}
fn print_agents(db: &Database) -> Result<()> {
    for a in db.list_agents().map_err(|e| anyhow::anyhow!(e))? {
        println!("{}\t{}", a.id, a.status);
    }
    Ok(())
}
fn print_synced_quota(id: &str, snapshot: &codex_app_server::QuotaSnapshot) {
    println!("{}:\n  remaining: {}%", id, snapshot.remaining_percent);
}
fn print_quota_limit(_label: &str, _limit: Option<&registry::QuotaLimit>) {}
