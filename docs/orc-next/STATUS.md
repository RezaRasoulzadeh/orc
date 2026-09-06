# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M08 — Experience dataset

**Current task:** M08-010 — Add explicit typed curation for Controller memory maintenance target selection

**Last completed:** M08-009 — Add explicit typed curation for Controller memory maintenance judgment

**Blocked by:** Nothing

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M06 and M07 are complete.
- M08 is the curated Controller experience dataset required by D-006 and remains distinct from runtime `MemoryKind::Experience`.
- M08-001 established the canonical typed/versioned experience-example substrate with explicit trusted creation and M08 metadata/bounds validation.
- M08-002 through M08-007 curate normal recommendation, recovery, planning, workflow intake, Plan review, and Plan revision boundaries.
- M08-008 is complete at `9c57dec99175b9caafd1cc98ef280578f7c24de3`, curating exact `ControllerMemoryCaptureInput` / `ControllerMemoryCaptureResult` under fixed capability `controller.memory_capture`.
- M08-009 is complete at `6af29dc8fdd3a444ce5903ce308289cdd882b370`, curating exact `ControllerMemoryMaintenanceInput` / `ControllerMemoryMaintenanceResult` under fixed capability `controller.memory_maintenance` while preserving production target-binding validation.
- Verification, accepted output, quality, correction, and provenance remain explicit trusted inputs; downstream execution/mutation/workflow outcomes do not automatically label examples.
- The next repository-grounded seam is Controller memory-maintenance target selection.
- `ControllerMemorySelectionInput` already contains the exact bounded inference packet: project ID, explicit current facts, deterministic candidate projection, candidate ordering, and omission/count metadata.
- `ControllerMemorySelectionResult::validate(input)` already requires `SelectTarget` to choose one exact supplied candidate; `NoTarget` is the only other result.
- M08-010 must preserve the exact supplied input rather than re-enumerating or refreshing memory during curation.
- Candidate construction, later target resolution, maintenance judgment, grants, mutation execution, and durable memory state remain outside the reasoning target.
- No automatic harvesting, generic curation framework, runtime-memory change, export, splitting/balancing, embeddings, fine-tuning, Python runtime dependency, model-specific behavior, provider fallback, or provider token hard cap is introduced.

## Immediate next action

Implement `tasks/M08-010.md`: explicit typed curation of one already-produced Controller memory-maintenance target-selection judgment into the canonical M08-001 dataset format, preserving the exact supplied candidate projection and reusing production target-membership validation.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
