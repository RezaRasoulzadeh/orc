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

M08-008 is complete. It curates exact `ControllerMemoryCaptureInput` / `ControllerMemoryCaptureResult` under fixed capability `controller.memory_capture`, with production candidate-backed validation authoritative and later mutation/grant/execution outside dataset labels.

M08-009 is complete at `6af29dc8fdd3a444ce5903ce308289cdd882b370`. It curates exact `ControllerMemoryMaintenanceInput` / `ControllerMemoryMaintenanceResult` under fixed capability `controller.memory_maintenance`, preserving exact target-bound Correct/Supersede/Remove validation and excluding target selection, grants, execution, and later memory state from the reasoning target.

M08-010 now targets Controller memory-maintenance target selection. The production reasoning boundary consumes exact `ControllerMemorySelectionInput`, containing current project ID, explicit current facts, a bounded deterministic candidate projection, candidate ordering, and eligible/selected/omitted counts. The output is exactly `NoTarget` or `SelectTarget { target }`, and production validation requires any selected identity to be one exact supplied candidate.

M08-010 must curate the exact already-supplied typed inference packet and result. It must not re-enumerate memory, rebuild candidate order, infer omitted candidates, refresh storage, call maintenance judgment, issue grants, execute mutations, or derive verification from downstream results.

After remaining unambiguous capability-local judgment seams are covered, later M08 work should shift to deterministic dataset inspection/export/evaluation preparation only as repository evidence warrants. No generic capture framework, embeddings, Python runtime dependency, automatic harvesting, provider fallback, or provider token hard cap is introduced.

## M09 — Controller specialization

Fine-tune/evaluate the local Controller model. A new model becomes default only when evaluation demonstrates improvement without unacceptable regressions.

## M10 — Interface integration

Expose the mature Controller consistently through CLI, TUI and GUI using shared core APIs.
