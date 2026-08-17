pub mod adoption;
pub mod agent;
pub mod backend;
pub mod codex_app_server;
pub mod contract;
pub mod discovery;
pub mod doctor;
pub mod git;
pub mod protocol;
pub mod registry;
pub mod state;
pub mod storage;
pub mod task;
pub mod validation;
pub mod worker;

// Re-export useful types for tests
pub use protocol::*;
pub use state::*;
pub use storage::Database;
pub use task::*;
pub use validation::{
    SystemValidationRunner, ValidationConfig, ValidationReport, ValidationRunner,
    ValidationStepResult, run_validation_pipeline,
};
pub use worker::*;
