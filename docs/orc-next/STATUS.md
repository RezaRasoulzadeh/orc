# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M06 — Persistent memory

**Current task:** M06-010 — Add supervised Controller memory capture judgment

**Last completed:** M06-009 — Establish supervised Controller memory mutation boundary

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one; initial runtime target remains Qwen3 8B through llama.cpp/GGUF behind a model-independent runtime boundary.
- `ProjectOperations` is the primary provider-independent observation seam; `OrcApp` is the canonical application/mutation seam.
- M02–M05 are complete: Controller owns supervised judgment while deterministic kernel code owns persistence, legality, authorization, workflow routing, validation, approval/application gates, and lifecycle invariants.
- Memory is explicit Orc data, separate from model weights.
- M06-001 established typed User/Project/Episodic/Experience durable memory and canonical project/global persistence.
- M06-002 established deterministic bounded read-only `ControllerMemoryContext` through `OrcApp`.
- Controller capabilities retain capability-specific request/state types; memory reuse occurs through `ControllerMemoryContext`, not a universal Controller packet.
- M06-003 through M06-008 integrate bounded memory into all currently identified Controller read/judgment seams while preserving current-facts authority and kernel boundaries.
- M06-009 established the supervised write boundary: bounded typed create/correct/supersede/remove intents, deterministic legality, opaque exact-intent one-shot authorization, fresh-state revalidation, and canonical execution exclusively through M06-001 `MemoryService` operations.
- M06-010 adds only the missing capture judgment: one explicit bounded candidate may be judged `Ignore` or proposed as one canonical M06-009 mutation intent. Capture inference cannot authorize or execute memory mutation.
- Capture remains explicitly invoked and supervised; no background/implicit capture, transcript ingestion, or consolidation.
- Structured facts and deterministic filters come first; semantic/vector retrieval remains later.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M06-010.md`: add read-only Controller memory-capture judgment over one explicit bounded candidate, reusing `ControllerMemoryContext` and the existing M06-009 mutation-intent boundary without authorization or execution.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and migration map.
