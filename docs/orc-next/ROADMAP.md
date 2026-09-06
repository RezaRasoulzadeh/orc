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

## M08 — Experience dataset — COMPLETE

M08 established the curated Controller experience dataset required by D-006 while keeping it distinct from runtime `MemoryKind::Experience`.

M08-001 established the canonical typed/versioned experience-example format, explicit trusted verification metadata, bounded validation, deterministic query, and lifecycle.

M08-002 through M08-010 added explicit capability-local curation for every current inference-backed Controller judgment boundary: normal recommendations, recovery, planning, workflow intake, Plan review, Plan revision, memory capture, memory maintenance, and memory-maintenance target selection.

M08-010 completed exact memory-selection curation at `87db2f24151d3522394f7ed9a2a7042aedf66b50`, preserving exact supplied candidate projections and production candidate-membership validation.

M08-011 completed deterministic read-only complete-dataset inventory at `6791f81e38cf9af6ac04d16d9a6d5998e3369337`. Inventory reads only canonical global-registry metadata, reports exact global/per-capability lifecycle, outcome, and verification counts, preserves exact capability strings, validates aggregate invariants, and fails closed on malformed persisted metadata.

M08 introduces no automatic harvesting, runtime-memory coupling, embeddings, provider fallback, provider token hard cap, trainer-specific export, or continuous weight mutation.

## M09 — Controller specialization — CURRENT

Fine-tune/evaluate the local Controller model. A new model becomes default only when evaluation demonstrates improvement without unacceptable regressions.

M09 begins with specialization preparation rather than immediately choosing a trainer or mutating model weights.

M09-001 targets one deterministic trainer-neutral snapshot of the complete Active canonical M08 dataset. The snapshot preserves exact canonical example fields in stable code-owned order and provides a reproducible handoff to later training/evaluation work.

M09-001 must not introduce trainer-specific prompt/chat transforms, train/validation/test splitting, balancing, sampling, deduplication, ranking, weighting, quality/readiness policy, model promotion, inference changes, Python runtime dependencies, embeddings, provider fallback, or provider token hard caps.

Later M09 work should use the trainer-neutral snapshot plus existing Controller evaluation surfaces to investigate a native/offline specialization path, establish an explicit baseline and promotion gate, perform controlled training/evaluation, and only then consider changing the default Controller model.

## M10 — Interface integration

Expose the mature Controller consistently through CLI, TUI and GUI using shared core APIs.
