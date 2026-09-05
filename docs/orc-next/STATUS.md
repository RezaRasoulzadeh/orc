# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M07 — Supervised autonomy

**Current task:** M07-008 — Establish explicit bounded Controller memory maintenance grants

**Last completed:** M07-007 — Compose one supervised Controller memory capture step

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
- M07-007 is complete. `OrcApp::capture_controller_memory_once(...)` composes one explicit capture request through M06-010 judgment → M06-009 proposal → M07-006 grant inspection → M06-009 execution, with at most one inference/proposal/authorization/execution attempt.
- M07-007 returns state-safe typed outcomes; Ignore and all pre-mint failures consume zero capture budget, successful authorization consumes one, and post-mint failure is not refunded.
- M07-007 does not derive capture candidates automatically. `ControllerMemoryCaptureRequest` contains a full `MemoryDraft`; deterministic workflow code must not synthesize durable memory content merely to enable automation because that would hardcode Controller judgment.
- M06-011 maintenance remains explicit-target read-only judgment and returns only Keep or exact-target Correct/Supersede/Remove proposals.
- The next safe M07 seam is a separate finite maintenance permission rather than broadening the capture or task continuation grants.
- M07-008 must authorize only already validated project-bound Project/Episodic Correct/Supersede/Remove proposals through the existing M06-009 exact one-shot authorization. Create and User/Experience remain excluded.
- M07-008 adds permission only: no automatic target selection, maintenance invocation, execution composition, scanning, workflow hooks, or background cleanup.
- Continuation-action, capture, and maintenance budgets are separate capability constraints; no provider token hard cap is introduced.
- Grants remain in-process and are not persisted/reconstructed after restart.
- Preserve deterministic validation truth and all existing canonical mutation paths.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M07-008.md`: add a finite project-bound Controller memory-maintenance grant that can mint the existing M06-009 one-shot authorization only for eligible Project/Episodic Correct/Supersede/Remove proposals, without automatic maintenance invocation or mutation-path duplication.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and migration map.
