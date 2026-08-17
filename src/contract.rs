use anyhow::{Context, Result};
use std::path::Path;

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
