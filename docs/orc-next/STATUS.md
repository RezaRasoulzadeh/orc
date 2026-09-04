# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M06 — Persistent memory

**Current task:** M06-008 — Integrate bounded memory into Controller Plan revision

**Last completed:** M06-007 — Integrate bounded memory into Controller Plan review

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one; initial runtime target remains Qwen3 8B through llama.cpp/GGUF behind a model-independent runtime boundary.
- `ProjectOperations` is the primary provider-independent observation seam; `OrcApp` is the canonical application/mutation seam.
- M02–M05 are complete: Controller owns supervised judgment while deterministic kernel code owns persistence, legality, authorization, workflow routing, validation, approval/application gates, and lifecycle invariants.
- Memory is explicit Orc data, separate from model weights.
- M06-001 established typed User/Project/Episodic/Experience durable memory and canonical project/global persistence.
- M06-002 established deterministic bounded read-only `ControllerMemoryContext` through `OrcApp`.
- Controller capabilities retain capability-specific request/state types; memory reuse occurs through `ControllerMemoryContext`, not a universal Controller packet.
- M06-003 through M06-007 integrate bounded memory into Plan generation, recovery, normal task recommendation, workflow intake, and Plan review respectively while preserving current-facts authority and kernel boundaries.
- M06-008 applies the same pattern to read-only Controller Plan revision. Current Plan content, persisted actionable revision feedback, and current planning facts remain authoritative; durable lineage remains outside model authority.
- Structured facts and deterministic filters come first; semantic/vector retrieval remains later.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M06-008.md`: integrate canonical bounded memory into the existing Controller Plan-revision inference seam while preserving deterministic eligibility, persisted feedback authority, canonical PlanResponse output, trusted lineage attachment, and downstream kernel-owned persistence/workflow semantics.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and migration map.
