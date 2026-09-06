# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M08 — Experience dataset

**Current task:** M08-011 — Add deterministic Controller experience dataset inventory

**Last completed:** M08-010 — Add explicit typed curation for Controller memory maintenance target selection

**Blocked by:** Nothing

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M06 and M07 are complete.
- M08 is the curated Controller experience dataset required by D-006 and remains distinct from runtime `MemoryKind::Experience`.
- M08-001 established the canonical typed/versioned experience-example substrate with explicit trusted creation, bounded validation, provenance, correction/outcome/quality metadata, deterministic query, and Active/Retired lifecycle.
- M08-002 through M08-010 now curate every current inference-backed Controller judgment boundary: normal recommendation, recovery, planning, workflow intake, Plan review, Plan revision, memory capture, memory maintenance, and memory-maintenance target selection.
- M08-009 is complete at `6af29dc8fdd3a444ce5903ce308289cdd882b370` with fixed capability `controller.memory_maintenance`.
- M08-010 is complete at `87db2f24151d3522394f7ed9a2a7042aedf66b50` with fixed capability `controller.memory_selection`, exact `ControllerMemorySelectionInput`, exact `ControllerMemorySelectionResult`, and production candidate-membership validation preserved.
- Verification, accepted output, quality, correction, and provenance remain explicit trusted inputs. Downstream execution/mutation/workflow outcomes do not automatically label examples.
- The repository now has no remaining uncovered current inference-backed Controller judgment module requiring another capability-local curation adapter.
- The next smallest evidence-backed M08 seam is deterministic dataset inspection: exact metadata counts over canonical global-registry M08-001 rows before export, splitting, balancing, evaluation preparation, or M09 specialization.
- M08-011 must be read-only and deterministic: complete dataset totals, Active/Retired counts, lexicographically sorted per-capability lifecycle/outcome/verification counts, aggregate invariant checks, and fail-closed handling of malformed persisted metadata.
- M08-011 must not reinterpret capability payloads, infer dataset quality/readiness, mutate examples, export trainer records, run Controller inference, read runtime Experience memory, or introduce Python.
- No automatic harvesting, generic curation framework, embeddings, model-specific behavior, provider fallback, or provider token hard cap is introduced.

## Immediate next action

Implement `tasks/M08-011.md`: a deterministic read-only inventory over the complete canonical Controller experience dataset, using persisted M08 metadata only and preserving the separation between dataset inspection and later export/training policy.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
