use anyhow::{Result, bail};
use clap::Subcommand;

use crate::app::OrcApp;
use crate::registry::EconomyTier;

#[derive(Clone, Debug, PartialEq)]
pub struct ModelCost {
    model: String,
    cost: f64,
}

fn parse_model_cost(value: &str) -> Result<ModelCost, String> {
    let (model, cost) = value
        .split_once('=')
        .ok_or_else(|| "model cost must use MODEL=COST".to_owned())?;
    let model = model.trim();
    if model.is_empty() {
        return Err("model cost requires a non-empty model name".into());
    }
    let cost = cost
        .trim()
        .parse::<f64>()
        .map_err(|_| "model cost must be a non-negative finite number".to_owned())?;
    if !cost.is_finite() || cost < 0.0 {
        return Err("model cost must be a non-negative finite number".into());
    }
    Ok(ModelCost {
        model: model.into(),
        cost,
    })
}

fn parse_economy_tier(value: &str) -> Result<EconomyTier, String> {
    EconomyTier::parse(value).map_err(|error| error.to_string())
}

#[derive(Subcommand)]
pub enum EconomyCommand {
    /// Show persisted tier configuration and project economy evidence as JSON.
    Show,
    /// Inspect size-only context attribution for one or all provider invocations.
    Context {
        /// Provider invocation ID. Omit to list every invocation oldest first.
        invocation_id: Option<i64>,
    },
    /// Merge model costs into the persisted project economy configuration.
    Configure {
        /// Relative model cost in MODEL=COST form; repeat for multiple models.
        #[arg(long = "model-cost", value_parser = parse_model_cost)]
        model_costs: Vec<ModelCost>,
        /// Tier assigned to models absent from the configured cost map.
        #[arg(long, value_parser = parse_economy_tier)]
        unknown_tier: Option<EconomyTier>,
    },
}

pub fn run(command: EconomyCommand, db_path: &str, repo_path: &str) -> Result<()> {
    let app = OrcApp::open_global(db_path, repo_path)?;
    match command {
        EconomyCommand::Show => {
            let configuration = app.database().economy_cost_configuration()?;
            let economy = app.economy_summary()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "configuration": configuration,
                    "economy": economy,
                }))?
            );
        }
        EconomyCommand::Context { invocation_id } => {
            let value = match invocation_id {
                Some(id) => serde_json::to_value(
                    app.provider_invocation_summary(id)?
                        .ok_or_else(|| anyhow::anyhow!("provider invocation {id} not found"))?,
                )?,
                None => serde_json::to_value(app.provider_invocation_summaries()?)?,
            };
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        EconomyCommand::Configure {
            model_costs,
            unknown_tier,
        } => {
            if model_costs.is_empty() && unknown_tier.is_none() {
                bail!("economy configure requires --model-cost or --unknown-tier");
            }
            let mut configuration = app.database().economy_cost_configuration()?;
            for item in model_costs {
                configuration.model_costs.insert(item.model, item.cost);
            }
            if let Some(tier) = unknown_tier {
                configuration.unknown_tier = tier;
            }
            app.database()
                .set_economy_cost_configuration(&configuration)?;
            println!("{}", serde_json::to_string_pretty(&configuration)?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_cost_parser_is_strict_and_preserves_model_name() {
        assert_eq!(
            parse_model_cost("model-small=0.75").unwrap(),
            ModelCost {
                model: "model-small".into(),
                cost: 0.75,
            }
        );
        assert!(parse_model_cost("model-small").is_err());
        assert!(parse_model_cost("=1").is_err());
        assert!(parse_model_cost("model-small=-1").is_err());
        assert!(parse_model_cost("model-small=NaN").is_err());
    }
}
