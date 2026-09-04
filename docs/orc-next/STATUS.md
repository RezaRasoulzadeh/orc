# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M06 — Persistent memory

**Current task:** M06-009 — Establish supervised Controller memory mutation boundary

**Last completed:** M06-008 — Integrate bounded memory into Controller Plan revision

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one; initial runtime target remains Qwen3 8B through llama.cpp/GGUF behind a model-independent runtime boundary.
- `ProjectOperations` is the primary provider-independent observation seam; `OrcApp` is the canonical application/mutation seam.
- M02–M05 are complete: Controller owns supervised judgment while deterministic kernel code owns persistence, legality, authorization, workflow routing, validation, approval/application gates, and lifecycle invariants.
- Memory is explicit Orc data, separate from model weights.
- M06-001 established typed User/Project/Episodic/Experience durable memory and canonical project/global persistence.
- M06-002 established deterministic bounded read-only `ControllerMemoryContext` through `OrcApp`.
- Controller capabilities retain capability-specific request/state types; memory reuse occurs through `ControllerMemoryContext`, not a universal Controller packet.
- M06-003 through M06-008 integrate bounded memory into Plan generation, recovery, normal task recommendation, workflow intake, Plan review, and Plan revision while preserving current-facts authority and kernel boundaries.
- All currently identified Controller read/judgment seams now receive bounded typed memory.
- M06-009 begins the write side by establishing supervised typed memory mutation intents plus deterministic legality, one-shot authorization, fresh-state revalidation, and canonical M06-001 execution. It does not decide what should be remembered and does not add autonomous capture/consolidation.
- Structured facts and deterministic filters come first; semantic/vector retrieval remains later.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M06-009.md`: establish the supervised Controller memory mutation boundary over existing M06-001 APIs without adding inference or autonomous memory writes.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and migration map.
