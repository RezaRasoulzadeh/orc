# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M07 — Supervised autonomy

**Current task:** M07-007 — Compose one supervised Controller memory capture step

**Last completed:** M07-006 — Establish explicit bounded Controller memory capture grants

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one; initial runtime target remains Qwen3 8B through llama.cpp/GGUF behind a model-independent runtime boundary.
- `ProjectOperations` is the primary provider-independent observation seam; `OrcApp` is the canonical application/mutation seam.
- M02–M05 are complete: Controller owns supervised judgment while deterministic kernel code owns persistence, legality, authorization, workflow routing, validation, approval/application gates, and lifecycle invariants.
- M06 is complete: memory remains explicit Orc data separate from model weights, with typed User/Project/Episodic/Experience persistence, deterministic bounded retrieval, capability-local Controller integration, supervised mutation, explicit-candidate capture judgment, and explicit-target maintenance judgment.
- `ControllerMemoryContext` remains the reusable bounded read-only memory projection; capability-specific requests retain their own current-fact/state types rather than converging on a universal packet.
- Existing M03 task actions already have typed intents, deterministic legality, exact one-shot trusted authorization, fresh legality at execution, and canonical mutation paths.
- Existing `WorkflowEngine` remains the one authoritative restart-safe continuation path and already owns finite revision/transition limits. M07 reuses it rather than introducing another loop.
- M07-001 through M07-005 complete bounded routine-task continuation: finite project-bound continuation grants, exact expected-action enforcement, one-edge workflow integration, and repeated grant-aware continuation through the existing workflow loop.
- `WorkflowEngine::route_tasks()` remains the sole task selector. Only Dispatch/Review/Revision consume routine continuation units; deterministic/routing/planning edges do not. Acceptance/user/external gates remain outside grant-authorized continuation.
- No additional routine-workflow continuation engine is needed.
- M06-009 remains the only canonical durable-memory mutation legality/authorization/execution path. M06-010 remains the one-candidate capture judgment seam and can only Ignore or propose one exact candidate-backed Create.
- M07-006 is complete. `ControllerMemoryCaptureGrant` is a distinct project-bound finite permission domain with a 128-action maximum, shared clone state, Active/Exhausted/Revoked lifecycle, explicit revocation, and no persistence/restart reconstruction.
- `OrcApp::inspect_controller_memory_capture_grant()` may mint only the existing M06-009 exact one-shot authorization for exact-current-project `Project` or `Episodic` Create proposals. User/Experience and Correct/Supersede/Remove remain ineligible.
- M07-006 consumes one capture-grant unit only after successful M06-009 authorization mint; pre-mint rejection consumes zero and post-mint failure is not refunded.
- M07-006 does not invoke capture judgment or derive candidates automatically.
- The next smallest M07 seam is one-step composition: explicitly supplied M06-010 capture request → judgment → canonical M06-009 proposal → M07-006 grant inspection → canonical M06-009 execution.
- M07-007 must perform at most one inference and one mutation attempt, with no retry/fallback/direct mutation. Ignore and all pre-mint failures consume zero; successful authorization consumes one; post-mint failure receives no refund.
- Automatic candidate derivation from workflow/task/Plan/review/validation/recovery/transcript/lifecycle events remains outside M07-007.
- Automatic User/Experience writes, automatic maintenance, background scanning, semantic/vector retrieval, and embeddings remain out of scope.
- Continuation-action budget, memory-capture budget, workflow transition/revision limits, scheduler permissions, quota/economy facts, and task lifecycle are independent kernel constraints. No provider token hard cap is reintroduced.
- Grants remain in-process and are not persisted or reconstructed after restart.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M07-007.md`: compose exactly one caller-supplied capture request through the existing M06-010 judgment, M06-009 proposal/execution, and M07-006 grant boundaries, without automatic candidate derivation or workflow-event integration.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and migration map.
