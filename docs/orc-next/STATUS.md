# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M08 — Experience dataset

**Current task:** M08-006 — Add explicit typed curation for Controller Plan review

**Last completed:** M08-005 — Add explicit typed curation for Controller workflow intake

**Blocked by:** Nothing

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M06 and M07 are complete.
- M08 is the curated Controller experience dataset required by D-006 and remains distinct from runtime `MemoryKind::Experience`.
- M08-001 established the canonical typed/versioned global-registry experience-example substrate with explicit trusted creation, bounded validation, provenance, correction/outcome/quality metadata, deterministic query, and active/retired lifecycle.
- M08-002 is complete at implementation `adfac96dd5c8f77ef7d627858beb8a9aa58ded3b`, providing explicit typed curation for normal task recommendation with fixed capability `controller.task_recommendation`.
- M08-003 is complete at implementation `947d2d8e9cf201e82daaa62474be74d26df7ef4f`, providing explicit typed curation for recovery recommendation with fixed capability `controller.recovery_recommendation`.
- M08-004 is complete at implementation `12dd31c0086ff2356ac3e5c75508ed6a4bab7438`, providing explicit typed curation for planning with fixed capability `controller.plan_generation` and exact complete `ControllerPlanningInput` / `ControllerPlanResult` projection.
- M08-005 is complete at implementation `f44af905e2e22beeddadd3253a1ecf2729bfe7d4`, providing explicit typed curation for workflow intake with fixed capability `controller.workflow_intake` and exact complete `ControllerIntakeInput` / `ControllerIntakeResult` projection.
- Verification, accepted output, quality, correction, and provenance remain explicit trusted inputs. Execution/workflow/review/validation success does not automatically label dataset examples.
- The next smallest repository-grounded seam is Controller Plan review. `ControllerPlanReviewInput` and `ControllerPlanReviewResult` already form a bounded typed input/output contract with reusable production validation and no mutation capability.
- M08-006 must preserve the exact Plan-review inference input and complete accepted semantic result. Persisted PlanReview rows, Plan status transitions, subsequent revision/application, workflow advancement, and downstream success must not automatically verify or correct examples.
- Plan revision remains a later seam because its public result contains trusted parent/review lineage attached after inference in addition to model-generated Plan content; that dataset boundary must be handled deliberately rather than copied from Plan review.
- No automatic harvesting, Plan-review/workflow hook, runtime-memory change, export, splitting/balancing, embeddings, fine-tuning, Python runtime dependency, model-specific behavior, provider fallback, or provider token hard cap is introduced.

## Immediate next action

Implement `tasks/M08-006.md`: explicit typed curation of one already-produced Controller Plan-review interaction into the canonical M08-001 dataset format.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
