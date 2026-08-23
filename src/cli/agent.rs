use crate::app::OrcApp;
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
        #[arg(long, value_parser = parse_agent_action)]
        action: Vec<registry::AgentAction>,
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
    Purge {
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
    Actions {
        id: String,
    },
    ActionAdd {
        id: String,
        #[arg(value_parser = parse_agent_action)]
        action: registry::AgentAction,
    },
    ActionRemove {
        id: String,
        #[arg(value_parser = parse_agent_action)]
        action: registry::AgentAction,
    },
}

pub fn run(command: AgentCommand, db_path: &str) -> Result<()> {
    let db = Database::open(db_path).map_err(|e| anyhow::anyhow!(e))?;
    let app = OrcApp::open(db_path, ".")?;
    match command {
        AgentCommand::List => {
            print_agents(&db)?;
        }
        AgentCommand::Add {
            id,
            backend,
            priority,
            capability,
            action,
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
                actions: if action.is_empty() {
                    vec![registry::AgentAction::Code]
                } else {
                    action
                },
            };
            app.configure_agent(agent.clone())?;
            println!("Added agent {}", agent.id);
        }
        AgentCommand::Enable { id } => {
            ensure_agent_updated(app.set_agent_enabled(&id, true)?, &id)?
        }
        AgentCommand::Disable { id } => {
            ensure_agent_updated(app.set_agent_enabled(&id, false)?, &id)?
        }
        AgentCommand::Remove { id } => {
            db.archive_agent(&id).map_err(|e| anyhow::anyhow!(e))?;
            println!("Archived agent {}", id);
        }
        AgentCommand::Purge { id } => {
            app.purge_agent(&id)?;
            println!("Purged agent {}", id);
        }
        AgentCommand::Unavailable { id, reason } => {
            ensure_agent_updated(app.set_agent_availability(&id, false, Some(&reason))?, &id)?;
        }
        AgentCommand::Available { id } => {
            ensure_agent_updated(app.set_agent_availability(&id, true, None)?, &id)?;
        }
        AgentCommand::Priority { id, priority } => {
            ensure_agent_updated(app.set_agent_priority(&id, priority)?, &id)?
        }
        AgentCommand::Profile { id, path } => {
            ensure_agent_updated(app.set_agent_profile(&id, &path)?, &id)?
        }
        AgentCommand::Model { id, model } => {
            ensure_codex_automated_agent(&db, &id)?;
            ensure_agent_updated(app.set_agent_model(&id, &model)?, &id)?;
        }
        AgentCommand::Effort { id, effort } => {
            ensure_codex_automated_agent(&db, &id)?;
            ensure_agent_updated(app.set_agent_effort(&id, effort)?, &id)?;
        }
        AgentCommand::Quota {
            id,
            remaining,
            reset,
        } => ensure_agent_updated(app.set_agent_quota(&id, remaining, reset.as_deref())?, &id)?,
        AgentCommand::QuotaClear { id } => ensure_agent_updated(app.clear_agent_quota(&id)?, &id)?,
        AgentCommand::QuotaReserve { remaining } => {
            app.set_quota_reserve(remaining)?;
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
                    .map_or_else(|| "-".into(), crate::format::timestamp)
            );
            println!(
                "Quota checked:       {}",
                agent
                    .quota_checked_at
                    .as_deref()
                    .map_or_else(|| "-".into(), crate::format::timestamp)
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
                        crate::format::timestamp(&limit.reset_at.to_string())
                    );
                }
            }
            println!("Capabilities:        {}", agent.capabilities.join(", "));
            println!(
                "Actions:             {}",
                agent
                    .actions
                    .iter()
                    .map(|action| action.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            println!(
                "Profile/config:      {}",
                agent.profile_path.as_deref().unwrap_or("-")
            );
        }
        AgentCommand::Actions { id } => {
            for profile in app.agent_action_profiles(&id)? {
                println!(
                    "{}\tmodel={}\teffort={}",
                    profile.action.as_str(),
                    profile.model.as_deref().unwrap_or("-"),
                    profile
                        .reasoning_effort
                        .map(|value| value.as_str())
                        .unwrap_or("-")
                );
            }
            if app.agent_action_profiles(&id)?.is_empty() {
                let agent = registry::get_agent(&db, &id)?;
                for action in agent.actions {
                    println!("{}\tmodel=-\teffort=-", action.as_str());
                }
            }
        }
        AgentCommand::ActionAdd { id, action } => {
            if !app.add_agent_action(&id, action)? {
                anyhow::bail!("agent '{}' is not registered", id);
            }
            println!("Added action {} to agent {}", action.as_str(), id);
        }
        AgentCommand::ActionRemove { id, action } => {
            if !app.remove_agent_action(&id, action)? {
                anyhow::bail!(
                    "agent '{}' does not support action '{}'",
                    id,
                    action.as_str()
                );
            }
            println!("Removed action {} from agent {}", action.as_str(), id);
        }
    }

    Ok(())
}

fn parse_reasoning_effort(value: &str) -> Result<registry::ReasoningEffort, String> {
    registry::ReasoningEffort::parse(value).map_err(|e| e.to_string())
}
fn parse_agent_action(value: &str) -> Result<registry::AgentAction, String> {
    registry::AgentAction::parse(value).map_err(|e| e.to_string())
}
fn ensure_agent_updated(changed: bool, id: &str) -> Result<()> {
    if !changed {
        anyhow::bail!("agent '{}' is not registered", id);
    }
    Ok(())
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
