# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M08 — Experience dataset

**Current task:** M08-007 — Add explicit typed curation for Controller Plan revision generation

**Last completed:** M08-006 — Add explicit typed curation for Controller Plan review

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
- M08-006 is complete at implementation `cbef7d8c82aca0c6364e9a8e8e453b56d863edcb`, providing explicit typed curation for Plan review with fixed capability `controller.plan_review` and exact `ControllerPlanReviewInput` / `ControllerPlanReviewResult` projection.
- Verification, accepted output, quality, correction, and provenance remain explicit trusted inputs. Execution/workflow/review/validation success does not automatically label dataset examples.
- The next smallest repository-grounded seam is Controller Plan revision generation.
- `ControllerPlanRevisionBuilder::revise_with_memory(...)` consumes exact `ControllerPlanRevisionInput` and the actual model-generated output is a canonical `PlanResponse`.
- `ControllerPlanRevisionResult` is not the dataset reasoning target because trusted application code attaches `parent_plan_id`, `parent_plan_version`, and `review_id` after inference.
- M08-007 must preserve the exact typed revision input and exact generated/accepted canonical `PlanResponse` while excluding trusted lineage from the reasoning target.
- Revised-Plan persistence, lineage consistency, later Plan review, task application, workflow advancement, and downstream success must not automatically verify or correct examples.
- No automatic harvesting, revision/workflow hook, runtime-memory change, export, splitting/balancing, embeddings, fine-tuning, Python runtime dependency, model-specific behavior, provider fallback, or provider token hard cap is introduced.

## Immediate next action

Implement `tasks/M08-007.md`: explicit typed curation of one already-produced Controller Plan-revision generation interaction into the canonical M08-001 dataset format, using generated `PlanResponse` as the reasoning output authority and excluding trusted lineage metadata.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
