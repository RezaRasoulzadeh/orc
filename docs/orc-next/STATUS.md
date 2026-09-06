# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M08 — Experience dataset

**Current task:** M08-009 — Add explicit typed curation for Controller memory maintenance judgment

**Last completed:** M08-008 — Add explicit typed curation for Controller memory capture judgment

**Blocked by:** Nothing

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M06 and M07 are complete.
- M08 is the curated Controller experience dataset required by D-006 and remains distinct from runtime `MemoryKind::Experience`.
- M08-001 established the canonical typed/versioned global-registry experience-example substrate with explicit trusted creation, bounded validation, provenance, correction/outcome/quality metadata, deterministic query, and active/retired lifecycle.
- M08-002 is complete at implementation `adfac96dd5c8f77ef7d627858beb8a9aa58ded3b`, with fixed capability `controller.task_recommendation`.
- M08-003 is complete at implementation `947d2d8e9cf201e82daaa62474be74d26df7ef4f`, with fixed capability `controller.recovery_recommendation`.
- M08-004 is complete at implementation `12dd31c0086ff2356ac3e5c75508ed6a4bab7438`, with fixed capability `controller.plan_generation`.
- M08-005 is complete at implementation `f44af905e2e22beeddadd3253a1ecf2729bfe7d4`, with fixed capability `controller.workflow_intake`.
- M08-006 is complete at implementation `cbef7d8c82aca0c6364e9a8e8e453b56d863edcb`, with fixed capability `controller.plan_review`.
- M08-007 is complete at implementation `5df4d44a71f080f89dcdcb60856783117b03c721`, with fixed capability `controller.plan_revision`, exact `ControllerPlanRevisionInput`, and generated `PlanResponse` as the reasoning output authority.
- M08-008 is complete at implementation `9c57dec99175b9caafd1cc98ef280578f7c24de3`, with fixed capability `controller.memory_capture`, exact `ControllerMemoryCaptureInput`, and exact production-validated `ControllerMemoryCaptureResult`.
- Verification, accepted output, quality, correction, and provenance remain explicit trusted inputs. Execution/workflow/review/validation/mutation success does not automatically label dataset examples.
- The next smallest repository-grounded seam is Controller memory maintenance judgment.
- `ControllerMemoryMaintenanceInput` already carries one exact resolved canonical target, bounded explicit current facts, and bounded memory context.
- `ControllerMemoryMaintenanceResult::validate(input)` already enforces exact target binding, rejects `Create`, permits only `Correct`/`Supersede`/`Remove`, and requires Correct/Supersede replacements to preserve target kind, scope, and subject.
- M08-009 must curate that judgment boundary only. Mutation proposal validation, maintenance grants, composed execution, later durable memory state, and maintenance-target selection remain outside the reasoning target.
- Maintenance-target selection remains a separate later M08 seam because it has its own bounded `ControllerMemorySelectionInput` / `ControllerMemorySelectionResult` contract and deterministic candidate-construction boundary.
- No automatic harvesting, maintenance/mutation hook, runtime-memory change, export, splitting/balancing, embeddings, fine-tuning, Python runtime dependency, model-specific behavior, provider fallback, or provider token hard cap is introduced.

## Immediate next action

Implement `tasks/M08-009.md`: explicit typed curation of one already-produced Controller memory-maintenance judgment into the canonical M08-001 dataset format, reusing exact target-bound production validation and keeping later selection/grant/execution outside the reasoning target.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
