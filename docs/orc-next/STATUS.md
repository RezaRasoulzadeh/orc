# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M07 — Supervised autonomy

**Current task:** M07-004 — Route one Controller workflow task edge through a continuation grant

**Last completed:** M07-003 — Constrain supervised continuation to one expected routine action

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one; initial runtime target remains Qwen3 8B through llama.cpp/GGUF behind a model-independent runtime boundary.
- `ProjectOperations` is the primary provider-independent observation seam; `OrcApp` is the canonical application/mutation seam.
- M02–M05 are complete: Controller owns supervised judgment while deterministic kernel code owns persistence, legality, authorization, workflow routing, validation, approval/application gates, and lifecycle invariants.
- M06 is complete: memory remains explicit Orc data separate from model weights, with typed User/Project/Episodic/Experience persistence, deterministic bounded retrieval, capability-local Controller integration, supervised mutation, explicit-candidate capture judgment, and explicit-target maintenance judgment.
- `ControllerMemoryContext` remains the reusable bounded read-only memory projection; capability-specific requests retain their own current-fact/state types rather than converging on a universal packet.
- Existing M03 task actions already have typed intents, deterministic legality, exact one-shot trusted authorization, fresh legality at execution, and canonical mutation paths.
- Existing `WorkflowEngine` remains the one authoritative restart-safe continuation path and already owns finite revision/transition limits. M07 must reuse it rather than introduce another loop.
- M07-001 established opaque project-bound finite continuation grants over only Dispatch/SemanticReview/Revise. Grant inspection checks current canonical legality, consumes one budget unit only when an exact M03 authorization is minted, and cannot grant Accept.
- M07-002 established `OrcApp::continue_controller_action_once()`, composing one existing Controller proposal → exact task check → M07-001 grant inspection → existing M03 execution. Each call performs at most one inference and one routine action with no retry or automatic continuation.
- M07-003 added trusted expected-action enforcement to that one-step seam. The caller supplies an existing action kind; mismatched Controller recommendations and `Accept` stop before grant inspection and consume zero budget.
- `WorkflowEngine::route_tasks()` already owns task selection. Its persisted `Dispatch`, `Review`, and `Revision` stages already imply the exact routine action; M07 must not add Controller-driven task enumeration or another scheduler.
- M07-004 integrates exactly one such task-stage edge with the existing continuation grant. The stage supplies the expected action, `current_task_id` supplies the task, and `AppWorkflowActions` maps successful M07-003 execution back into the workflow's existing `ProviderOutcome` / `ReviewOutcome` transition logic.
- M07-004 does not pass grants through the existing multi-step `continue_run_with_controller_runtime`; repeated budget spending remains a later decision.
- Acceptance remains outside routine continuation grants and keeps existing workflow policy/user-gate semantics.
- Continuation cannot bypass scheduler/agent permissions, quota/economy facts, task lifecycle, validation/review evidence, revision requirements, workflow limits, or fresh M03 legality.
- No provider token hard cap is reintroduced; provider usage remains optimized and observable.
- Automatic memory capture/maintenance remains separate from the routine action continuation seam.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M07-004.md`: add one opt-in grant-aware `WorkflowEngine` single-edge continuation path that reuses the trusted current task stage as the expected M07-003 action and preserves existing workflow transition logic without adding a second loop or fallback execution path.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and migration map.
