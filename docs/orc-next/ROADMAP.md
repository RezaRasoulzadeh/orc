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

M09-001 completed the deterministic trainer-neutral specialization handoff at `481fe624777a03dc841ae1742dd5f9461e854fc7`. The snapshot reads and validates the complete canonical global-registry experience set in ascending example identity order, returns only Active examples, preserves the full canonical M08 record without capability-specific transformation, serializes deterministically, and performs zero writes or inference.

The next prerequisite is evaluation coverage that matches the actual Controller capability surface. The original M02 evaluation corpus established typed semantic evaluation for normal recommendations, but later milestones added recovery, planning, workflow intake, Plan review/revision, memory capture, memory maintenance, and maintenance-target selection.

M09-002 therefore establishes a deterministic model-independent specialization evaluation suite over all nine current inference-backed Controller capabilities. Expected results remain explicit fixture authority, evaluation compares typed semantics rather than prose, fake-runtime execution is deterministic and read-only, and global/per-capability result accounting must be exact.

M09-002 must not modify production prompts/parsers/runtime settings, infer expected answers from the training snapshot, choose a trainer, split/balance/sample the dataset, define promotion thresholds, or mutate model weights.

Later M09 work should use the trainer-neutral snapshot plus the full-surface evaluation suite to investigate a native/offline specialization path, define an explicit candidate-vs-baseline promotion gate, perform controlled training/evaluation, and only then consider changing the default Controller model.

## M10 — Interface integration

Expose the mature Controller consistently through CLI, TUI and GUI using shared core APIs.
