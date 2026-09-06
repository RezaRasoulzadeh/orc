# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M08 — Experience dataset

**Current task:** M08-004 — Add explicit typed curation for Controller planning results

**Last completed:** M08-003 — Add explicit typed curation for Controller recovery recommendations

**Blocked by:** Nothing

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M06 and M07 are complete.
- M08 is the curated Controller experience dataset required by D-006 and remains distinct from runtime `MemoryKind::Experience`.
- M08-001 established the canonical typed/versioned global-registry experience-example substrate with explicit trusted creation, bounded validation, provenance, correction/outcome/quality metadata, deterministic query, and active/retired lifecycle.
- M08-002 is complete at implementation `adfac96dd5c8f77ef7d627858beb8a9aa58ded3b`, providing explicit typed curation for normal task recommendation with fixed capability `controller.task_recommendation`.
- M08-003 is complete at implementation `947d2d8e9cf201e82daaa62474be74d26df7ef4f`, providing explicit typed curation for recovery recommendation with fixed capability `controller.recovery_recommendation`.
- M08-003 preserves exact validated `RecoveryInferenceInput` and `RecoveryRecommendation`, keeps `RecoveryRecommendationValidation` as runtime legality evidence rather than dataset target, and uses M08-001 as the only persistence path.
- Verification, accepted output, quality, correction, and provenance remain explicit trusted inputs. Execution/workflow/review/validation success does not automatically label dataset examples.
- The next smallest proven seam is Controller planning. `ControllerPlanningInput` and `ControllerPlanResult` already form a bounded typed input/output contract with reusable validation, so M08-004 can remain capability-local without a generic experience framework.
- M08-004 must preserve the exact planning input and complete accepted `ControllerPlanResult`, including its `PlanResponse`, rationale, and optional uncertainty. Plan persistence/review/workflow outcomes are not dataset targets and must not automatically label examples.
- No automatic harvesting, workflow/planner hook, runtime-memory change, export, splitting/balancing, embeddings, fine-tuning, Python runtime dependency, model-specific behavior, provider fallback, or provider token hard cap is introduced.

## Immediate next action

Implement `tasks/M08-004.md`: explicit typed curation of one already-produced Controller planning interaction into the canonical M08-001 dataset format.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
