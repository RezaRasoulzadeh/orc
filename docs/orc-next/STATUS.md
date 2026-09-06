# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M08 — Experience dataset

**Current task:** M08-008 — Add explicit typed curation for Controller memory capture judgment

**Last completed:** M08-007 — Add explicit typed curation for Controller Plan revision generation

**Blocked by:** Nothing

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M06 and M07 are complete.
- M08 is the curated Controller experience dataset required by D-006 and remains distinct from runtime `MemoryKind::Experience`.
- M08-001 established the canonical typed/versioned global-registry experience-example substrate with explicit trusted creation, bounded validation, provenance, correction/outcome/quality metadata, deterministic query, and active/retired lifecycle.
- M08-002 is complete at implementation `adfac96dd5c8f77ef7d627858beb8a9aa58ded3b`, providing explicit typed curation for normal task recommendation with fixed capability `controller.task_recommendation`.
- M08-003 is complete at implementation `947d2d8e9cf201e82daaa62474be74d26df7ef4f`, providing explicit typed curation for recovery recommendation with fixed capability `controller.recovery_recommendation`.
- M08-004 is complete at implementation `12dd31c0086ff2356ac3e5c75508ed6a4bab7438`, providing explicit typed curation for planning with fixed capability `controller.plan_generation`.
- M08-005 is complete at implementation `f44af905e2e22beeddadd3253a1ecf2729bfe7d4`, providing explicit typed curation for workflow intake with fixed capability `controller.workflow_intake`.
- M08-006 is complete at implementation `cbef7d8c82aca0c6364e9a8e8e453b56d863edcb`, providing explicit typed curation for Plan review with fixed capability `controller.plan_review`.
- M08-007 is complete at implementation `5df4d44a71f080f89dcdcb60856783117b03c721`, providing explicit typed curation for Plan revision generation with fixed capability `controller.plan_revision`, exact `ControllerPlanRevisionInput`, and generated canonical `PlanResponse` as the reasoning output authority while excluding trusted lineage.
- Verification, accepted output, quality, correction, and provenance remain explicit trusted inputs. Execution/workflow/review/validation/mutation success does not automatically label dataset examples.
- The next smallest repository-grounded seam is Controller memory capture judgment.
- `ControllerMemoryCaptureInput` is a bounded typed inference input containing one explicit candidate and bounded memory context. `ControllerMemoryCaptureResult` is exactly `Ignore` or one candidate-backed `ProposeMutation` result.
- Production `ControllerMemoryCaptureResult::validate(candidate)` already enforces that any proposed mutation is exactly one create intent preserving the explicit candidate draft.
- M08-008 must curate that judgment boundary only. Mutation proposal validation, capture grants, composed capture execution, later durable memory state, and downstream workflow outcomes must not automatically verify or correct examples.
- Memory maintenance judgment and maintenance-target selection remain later seams.
- No automatic harvesting, capture/mutation hook, runtime-memory change, export, splitting/balancing, embeddings, fine-tuning, Python runtime dependency, model-specific behavior, provider fallback, or provider token hard cap is introduced.

## Immediate next action

Implement `tasks/M08-008.md`: explicit typed curation of one already-produced Controller memory-capture judgment into the canonical M08-001 dataset format, reusing exact candidate-backed production validation and keeping later mutation/grant/execution outside the reasoning target.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
