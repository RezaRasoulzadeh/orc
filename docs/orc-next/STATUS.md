# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M07 — Supervised autonomy

**Current task:** M07-009 — Compose one supervised Controller memory maintenance step

**Last completed:** M07-008 — Establish explicit bounded Controller memory maintenance grants

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one; initial runtime target remains Qwen3 8B through llama.cpp/GGUF behind a model-independent runtime boundary.
- `ProjectOperations` is the primary provider-independent observation seam; `OrcApp` is the canonical application/mutation seam.
- M02–M05 are complete: Controller owns supervised judgment while deterministic kernel code owns persistence, legality, authorization, workflow routing, validation, approval/application gates, and lifecycle invariants.
- M06 is complete: memory remains explicit Orc data separate from model weights, with typed User/Project/Episodic/Experience persistence, deterministic bounded retrieval, capability-local Controller integration, supervised mutation, explicit-candidate capture judgment, and explicit-target maintenance judgment.
- `ControllerMemoryContext` remains the reusable bounded read-only memory projection; capability-specific requests retain their own current-fact/state types rather than converging on a universal packet.
- M07-001 through M07-005 complete bounded routine-task continuation through the existing `WorkflowEngine`; Acceptance/user/external gates remain outside grant-authorized continuation.
- M06-009 remains the only canonical durable-memory mutation legality/authorization/execution path.
- M07-006 established a distinct finite `ControllerMemoryCaptureGrant` for exact-current-project Project/Episodic Create proposals only.
- M07-007 is complete. `OrcApp::capture_controller_memory_once(...)` composes one explicit capture request through M06-010 judgment → M06-009 proposal → M07-006 grant inspection → M06-009 execution, with at most one inference/proposal/authorization/execution attempt and state-safe typed results.
- Automatic capture candidate derivation remains unresolved because `ControllerMemoryCaptureRequest` already contains a full `MemoryDraft`; deterministic workflow code must not synthesize durable memory content merely to enable automation.
- M07-008 is complete. `ControllerMemoryMaintenanceGrant` is a separate finite permission domain for already validated exact-current-project Project/Episodic Correct/Supersede/Remove proposals. Create and User/Experience remain excluded.
- M07-008 grants are opaque, in-process, clone-shared, revocable, capped at 128 actions, and non-persistent. Successful M06-009 authorization mint consumes one; pre-mint rejection consumes zero; post-mint execution failure is not refunded.
- M06-011 remains the sole explicit-target maintenance judgment seam and returns only Keep or one exact-target Correct/Supersede/Remove proposal.
- The next smallest M07 seam is one-step maintenance composition: explicit M06-011 request → judgment → canonical M06-009 proposal → M07-008 grant inspection → canonical M06-009 execution.
- M07-009 must perform at most one inference, one proposal, one authorization mint, and one execution attempt, with no retry/fallback/direct mutation. Keep and all pre-mint failures consume zero; successful authorization consumes one; post-mint failure receives no refund.
- M07-009 must use state-safe public result types and must not repeat the M07-007 impossible-state issue.
- Automatic maintenance target selection, scanning, workflow hooks, background cleanup, capture derivation, global User/Experience mutation, semantic/vector retrieval, and embeddings remain out of scope.
- Continuation-action, capture, and maintenance budgets are separate capability constraints; no provider token hard cap is introduced.
- Grants remain in-process and are not persisted/reconstructed after restart.
- Preserve deterministic validation truth and all existing canonical mutation paths.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M07-009.md`: compose exactly one caller-supplied maintenance request through existing M06-011 judgment, M06-009 proposal/execution, and M07-008 grant boundaries, without automatic target selection or mutation-path duplication.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and migration map.
