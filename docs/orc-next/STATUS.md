# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M07 — Supervised autonomy

**Current task:** Not yet decomposed

**Last completed:** M06-011 — Add supervised Controller memory maintenance judgment

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one; initial runtime target remains Qwen3 8B through llama.cpp/GGUF behind a model-independent runtime boundary.
- `ProjectOperations` is the primary provider-independent observation seam; `OrcApp` is the canonical application/mutation seam.
- M02–M05 are complete: Controller owns supervised judgment while deterministic kernel code owns persistence, legality, authorization, workflow routing, validation, approval/application gates, and lifecycle invariants.
- M06 is complete: memory remains explicit Orc data separate from model weights, with typed User/Project/Episodic/Experience persistence, deterministic bounded retrieval, capability-local Controller integration, supervised mutation, explicit-candidate capture judgment, and explicit-target maintenance judgment.
- `ControllerMemoryContext` remains the reusable bounded read-only memory projection; capability-specific requests retain their own current-fact/state types rather than converging on a universal packet.
- M06 mutation remains supervised: Controller judgments may propose Create/Correct/Supersede/Remove, while deterministic M06-009 legality, one-shot authorization, fresh-state validation, and `MemoryService` execution remain authoritative.
- Capture and maintenance are explicitly invoked. M06 intentionally adds no background scanning, implicit writes, transcript ingestion, autonomous consolidation, semantic/vector retrieval, embeddings, or learned ranking.
- Automatic invocation/continuation decisions now belong to M07 supervised-autonomy design and must reuse existing Controller/kernel boundaries rather than bypassing them.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Inspect the existing continuation, permission, approval, budget/economy, Controller-action, and workflow boundaries before defining the first narrow M07 supervised-autonomy task. Do not add a generic autonomous loop until the smallest safe continuation seam is identified.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and migration map.
