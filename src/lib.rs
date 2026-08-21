pub mod adoption;
pub mod agent;
pub mod backend;
pub mod cli;
pub mod codex_app_server;
pub mod contract;
pub mod discovery;
pub mod doctor;
pub mod git;
pub mod protocol;
pub mod queue;
pub mod registry;
pub mod review;
pub mod scheduler;
pub mod state;
pub mod storage;
pub mod task;
pub mod validation;
pub mod worker;

// Re-export useful types for tests
pub use protocol::*;
pub use queue::{
    BlockingReason, DependencyInfo, QueueCategory, QueueEntry, QueueItem, QueueReport,
    compute_queue,
};
pub use scheduler::{
    CandidateEvaluation, CandidateStatus, RejectionReason, ScheduleDecision, SelectionReason,
    evaluate_candidate, is_backend_mode_supported, schedule, validate_override,
};
pub use state::*;
pub use storage::Database;
pub use task::*;
pub use validation::{
    SystemValidationRunner, ValidationConfig, ValidationReport, ValidationRunner,
    ValidationStepResult, run_validation_pipeline,
};
pub use worker::*;
