pub mod adoption;
pub mod agent;
pub mod agent_onboarding;
pub mod app;
pub mod automated;
pub mod backend;
pub mod cli;
pub mod codex_app_server;
pub mod contract;
pub mod controller;
pub mod controller_actions;
pub mod controller_continuation;
pub mod controller_evaluation;
pub mod controller_experience;
pub mod controller_intake;
pub mod controller_memory;
pub mod controller_memory_capture;
pub mod controller_memory_capture_grant;
pub mod controller_memory_maintenance;
pub mod controller_memory_maintenance_grant;
pub mod controller_memory_mutation;
pub mod controller_memory_selection;
pub mod controller_memory_selection_maintenance;
pub mod controller_plan_persistence;
pub mod controller_plan_review;
pub mod controller_plan_review_persistence;
pub mod controller_plan_revision;
pub mod controller_plan_revision_persistence;
pub mod controller_planning;
pub mod desktop;
pub mod discovery;
pub mod doctor;
pub mod events;
pub mod execution;
pub mod execution_packet;
pub mod format;
pub mod git;
pub mod interactive;
pub mod lead;
pub mod local_runtime;
pub mod memory;
pub mod operations;
pub mod protocol;
pub mod provider_context;
pub mod queue;
pub mod read_model;
pub mod recovery;
pub mod recovery_controller;
pub mod recovery_execution;
pub mod registry;
pub mod review;
pub mod runtime;
pub mod scheduler;
pub mod self_hosting;
pub mod state;
pub mod storage;
pub mod task;
pub mod tui;
pub mod validation;
pub mod worker;
pub mod worker_protocol;
pub mod workflow;

// Re-export useful types for tests
pub use controller_memory::{
    ControllerMemoryAuthority, ControllerMemoryContext, ControllerMemoryError, ControllerMemoryItem,
};
pub use memory::{
    MemoryDraft, MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind,
    MemoryQuery, MemoryRecord, MemoryScope, MemoryService,
};
pub use protocol::*;
pub use queue::{
    BlockingReason, DependencyInfo, QueueCategory, QueueEntry, QueueItem, QueueReport,
    compute_queue,
};
pub use registry::{
    AGENT_MODEL_VERSION, Agent, AgentCapability, AgentExecution, AgentExecutionMode,
    AgentLifecycleState, AgentProviderConfiguration, AgentRole, GLOBAL_AGENT_SCOPE,
    OperatorPermission,
};
pub use scheduler::{
    CandidateEvaluation, CandidateStatus, RejectionReason, ScheduleDecision, SelectionReason,
    evaluate_candidate, is_backend_mode_supported, schedule, validate_override,
};
pub use state::*;
pub use storage::{Database, ProjectAgentReference};
pub use task::*;
pub use validation::{
    SystemValidationRunner, ValidationCategory, ValidationConfig, ValidationReport,
    ValidationRunner, ValidationStepResult, run_validation_pipeline,
};
pub use worker::*;
