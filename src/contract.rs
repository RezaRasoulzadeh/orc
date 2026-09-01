use anyhow::{Context, Result};
use std::path::Path;

pub const DEFAULT_ENGINEERING_CONTRACT: &str = include_str!("../.orc/engineering.md");

/// Provider-neutral starter contract for repositories newly adopted by Orc.
///
/// Orc's own maintained constitution is intentionally separate: copying its
/// product-specific storage, UI, and provider rules into an unrelated project
/// pollutes every worker packet with false constraints.
pub const DEFAULT_ADOPTED_PROJECT_ENGINEERING_CONTRACT: &str =
    include_str!("../assets/adopted-project-engineering.md");

/// Load the engineering contract from the specified path.
/// Returns the contract contents as a string.
pub fn load_contract<P: AsRef<Path>>(path: P) -> Result<String> {
    let path = path.as_ref();
    std::fs::read_to_string(path).with_context(|| {
        format!(
            "failed to load engineering contract from {}; ensure the file exists",
            path.display()
        )
    })
}
