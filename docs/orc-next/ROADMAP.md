# Orc Next Roadmap

The roadmap defines direction, not a frozen implementation plan. Only the current and next milestone should be decomposed deeply.

## M00 — Architecture and repository mapping — COMPLETE

Mapped the repository against the Controller/kernel target and established canonical application/observation seams.

## M01 — Native model runtime — COMPLETE

Model-independent local runtime with llama.cpp/GGUF Qwen3 8B integration.

## M02 — Read-only Controller — COMPLETE

Bounded project/task state and structured read-only Controller recommendations.

## M03 — Typed Controller tools — COMPLETE

Typed intents, deterministic legality, explicit authorization, canonical execution.

## M04 — Recovery intelligence — COMPLETE

Controller recovery judgment over deterministic failure/recovery facts.

## M05 — Planning and Lead unification — COMPLETE

Controller planning/intake/Plan review/revision with deterministic persistence and workflow gates.

## M06 — Persistent memory — COMPLETE

Typed User/Project/Episodic/Experience persistence, deterministic bounded `ControllerMemoryContext`, capability-local integration, supervised mutation, explicit-candidate capture judgment, and explicit-target maintenance judgment. Memory remains separate from model weights.

## M07 — Supervised autonomy — COMPLETE

Finite routine grants and supervised continuation are complete. Memory capture/maintenance grants, one-step composition, bounded maintenance-target selection, and selected-target maintenance composition are complete without bypassing deterministic mutation/authorization boundaries.

## M08 — Experience dataset — CURRENT

Turn verified Controller decisions, corrections, and outcomes into a curated evaluation/training dataset as required by D-006. Curated examples remain distinct from runtime `MemoryKind::Experience`.

M08-001 established the canonical typed/versioned experience-example format, explicit trusted verification metadata, bounded validation, deterministic query, and lifecycle.

M08-002 through M08-007 established capability-local curation for normal recommendations, recovery, planning, workflow intake, Plan review, and Plan revision generation.

M08-008 completed exact Controller memory-capture curation under fixed capability `controller.memory_capture`.

M08-009 completed exact Controller memory-maintenance curation at `6af29dc8fdd3a444ce5903ce308289cdd882b370` under fixed capability `controller.memory_maintenance`, preserving target-bound production validation.

M08-010 completed exact Controller memory-maintenance target-selection curation at `87db2f24151d3522394f7ed9a2a7042aedf66b50` under fixed capability `controller.memory_selection`, preserving exact supplied candidate projections and production candidate-membership validation.

With M08-010 complete, every current inference-backed Controller judgment module has an explicit capability-local M08 curation adapter. The next M08 work should therefore move from capture coverage to deterministic dataset inspection and evaluation preparation rather than inventing another reasoning seam.

M08-011 now targets a deterministic read-only inventory of the canonical global-registry experience dataset. It should report exact complete-dataset totals plus lexicographically sorted per-capability lifecycle, outcome, and verification-basis counts, validate aggregate invariants, and fail closed on malformed persisted metadata.

M08-011 must not reinterpret input/output payload semantics, infer training readiness, rank or balance examples, mutate lifecycle, export trainer formats, run Controller inference, read runtime Experience memory, or introduce Python.

Later M08 work may add deterministic export/evaluation preparation only after inspection establishes the concrete dataset shape. Train/validation/test splitting, balancing, sampling, deduplication, weighting, trainer-specific transforms, and specialization policy remain separate decisions and should not be bundled into the inventory task.

## M09 — Controller specialization

Fine-tune/evaluate the local Controller model. A new model becomes default only when evaluation demonstrates improvement without unacceptable regressions.

## M10 — Interface integration

Expose the mature Controller consistently through CLI, TUI and GUI using shared core APIs.
