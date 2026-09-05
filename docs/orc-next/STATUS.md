# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M07 — Supervised autonomy

**Current task:** M07-005 — Continue Controller workflows across routine task edges within one finite grant

**Last completed:** M07-004 — Route one Controller workflow task edge through a continuation grant

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
- M07-004 is complete. `WorkflowEngine::continue_one_with_controller_grant()` now routes exactly one persisted Controller task edge through M07-003/M07-001/M03 and maps successful canonical evidence back into the workflow's existing provider/review outcomes. There is no fallback provider execution after supervised rejection/failure.
- M07-004 keeps Acceptance and non-task stages outside its grant-aware single-edge entry point. Existing ordinary workflow APIs retain their prior semantics.
- The next repository-grounded seam is repeated bounded continuation using the existing `continue_run_inner()` loop. The same finite grant should apply only when that loop reaches Dispatch/Review/Revision; deterministic routing edges consume no grant.
- M07-005 must stop at Acceptance/user/external gates and must not automatically execute Accept even when the ordinary workflow policy is automatic. That restriction applies only to the new grant-aware multi-edge API; existing non-grant workflow behavior remains unchanged.
- Continuation cannot bypass scheduler/agent permissions, quota/economy facts, task lifecycle, validation/review evidence, revision requirements, workflow limits, or fresh M03 legality.
- Continuation-action budget and workflow transition/revision limits are independent kernel constraints; no provider token hard cap is reintroduced.
- Grants remain in-process and are not persisted or reconstructed after restart.
- Automatic memory capture/maintenance remains separate from the routine action continuation seam.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M07-005.md`: reuse the existing bounded workflow loop to continue across multiple persisted routine task edges under one finite continuation grant, while deterministic edges consume no grant and Acceptance/user/external boundaries stop supervised continuation.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and migration map.
