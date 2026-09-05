# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M07 — Supervised autonomy

**Current task:** M07-001 — Establish explicit bounded Controller continuation grants

**Last completed:** M06-011 — Add supervised Controller memory maintenance judgment

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one; initial runtime target remains Qwen3 8B through llama.cpp/GGUF behind a model-independent runtime boundary.
- `ProjectOperations` is the primary provider-independent observation seam; `OrcApp` is the canonical application/mutation seam.
- M02–M05 are complete: Controller owns supervised judgment while deterministic kernel code owns persistence, legality, authorization, workflow routing, validation, approval/application gates, and lifecycle invariants.
- M06 is complete: memory remains explicit Orc data separate from model weights, with typed User/Project/Episodic/Experience persistence, deterministic bounded retrieval, capability-local Controller integration, supervised mutation, explicit-candidate capture judgment, and explicit-target maintenance judgment.
- `ControllerMemoryContext` remains the reusable bounded read-only memory projection; capability-specific requests retain their own current-fact/state types rather than converging on a universal packet.
- Existing M03 task actions already have typed intents, deterministic legality, exact one-shot trusted authorization, fresh legality at execution, and canonical mutation paths.
- Existing `WorkflowEngine` remains the one authoritative restart-safe continuation path and already owns finite revision/transition limits. M07 must reuse it rather than introduce another loop.
- M07-001 establishes the missing operator-supervision contract: a project-bound finite continuation grant may permit only routine Dispatch/SemanticReview/Revise authorization. Accept remains explicitly authorized and is not grantable in this task.
- Continuation grants cannot bypass scheduler/agent permissions, quota/economy facts, task lifecycle, validation/review evidence, revision requirements, workflow limits, or fresh M03 legality.
- No provider token hard cap is reintroduced; provider usage remains optimized and observable.
- Automatic memory capture/maintenance remains out of M07-001.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M07-001.md`: establish an explicit project-bound finite continuation grant and deterministic inspection/authorization seam over existing Controller task actions, without adding an autonomous loop or automatic acceptance.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and migration map.
