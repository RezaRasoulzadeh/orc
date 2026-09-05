# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M07 — Supervised autonomy

**Current task:** M07-006 — Establish explicit bounded Controller memory capture grants

**Last completed:** M07-005 — Continue Controller workflows across routine task edges within one finite grant

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
- `WorkflowEngine::route_tasks()` remains the sole task selector for workflow continuation. Persisted `Dispatch`, `Review`, and `Revision` stages imply the exact routine action.
- M07-004 established the single-edge workflow adapter: `WorkflowEngine::continue_one_with_controller_grant()` routes exactly one persisted Controller task edge through M07-003/M07-001/M03 and maps canonical execution evidence back into existing workflow outcomes without fallback.
- M07-005 is complete. `WorkflowEngine::continue_run_with_controller_grant()` reuses the existing bounded `continue_run_inner()` loop to continue across multiple persisted routine task edges under one finite grant.
- Under M07-005, deterministic/routing/planning transitions consume zero continuation units; only Dispatch/Review/Revision can consume one unit each. Exhaustion/revocation/wrong-project/invalidity stops before another routine inference/action with no provider fallback.
- M07-005 keeps Acceptance outside supervised continuation even under ordinary automatic-acceptance policy; existing non-grant APIs retain their prior behavior. WaitingUser, WaitingExternal, transition budget, task revision limits, and restart-safe workflow persistence remain independent authoritative bounds.
- No additional routine-workflow continuation engine is needed after M07-005.
- M06-009 remains the only canonical durable-memory mutation legality/authorization/execution path. M06-010 capture can only Ignore or propose one exact candidate-backed Create; it cannot authorize or execute.
- The next M07 seam is a capability-specific finite memory-capture permission, not an extension of `ControllerContinuationGrant`. Routine task actions and durable memory mutation remain separate permission domains.
- M07-006 is restricted to project-bound Project/Episodic Create proposals. User/Experience global writes and Correct/Supersede/Remove maintenance remain outside automatic-capable capture permission.
- M07-006 must mint only the existing M06-009 exact one-shot memory authorization after deterministic eligibility checks; fresh M06-009 validation and `MemoryService` mutation remain authoritative.
- Automatic candidate derivation/invocation, workflow-event capture, background scanning, and automatic maintenance remain out of M07-006.
- Continuation-action budget, future memory-capture budget, workflow transition/revision limits, scheduler permissions, quota/economy facts, and task lifecycle are independent kernel constraints. No provider token hard cap is reintroduced.
- Grants remain in-process and are not persisted or reconstructed after restart.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M07-006.md`: add an explicit finite project-bound Controller memory-capture grant that can mint the existing M06-009 one-shot authorization only for eligible Project/Episodic Create proposals, without automatic invocation or mutation-path duplication.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and migration map.
