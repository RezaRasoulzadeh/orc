# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M06 — Persistent memory

**Current task:** M06-003 — Integrate bounded memory into Controller planning

**Last completed:** M06-002 — Build deterministic bounded Controller memory context

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one.
- Initial model/runtime target: Qwen3 8B through llama.cpp/GGUF.
- Qwen/llama.cpp is an implementation choice, not a domain dependency; the runtime boundary must remain replaceable.
- `ProjectOperations` is the primary existing provider-independent observation seam.
- `OrcApp` is the existing canonical application/mutation seam.
- M02 is complete: bounded read-only Controller state/recommendation and reliable structured output are in place.
- M03 is complete: typed normal-action intents, deterministic legality, trusted one-shot authorization, fresh legality re-check, canonical execution, and supervised recommendation-to-intent bridge are in place.
- M04 is complete: bounded recovery facts/legality, Controller recovery choice, supervised recovery execution, validation-repair exhaustion migration, and semantic revision non-convergence migration are in place.
- M05 is complete: the normal supervised Controller workflow owns intake plus Plan generation/review/revision judgment; deterministic kernel code owns persistence, workflow routing, approval/application gates, validation, authorization, and lifecycle invariants.
- Remaining legacy Lead/Planner APIs and durable records are compatibility-only; do not add cleanup tasks solely to delete them.
- Memory is explicit Orc data, separate from model weights.
- M06-001 is complete: typed User/Project/Episodic/Experience memory, canonical project/global persistence, transactional lifecycle/history operations, and application-level `MemoryService` are established.
- M06-002 is complete: standalone bounded `ControllerMemoryContext` retrieval through `OrcApp`, with deterministic active-only projection, project/global scope enforcement, explicit authority/provenance, and no inference integration.
- Controller capabilities keep capability-specific request/state types; memory reuse happens through `ControllerMemoryContext`, not by forcing a universal `ControllerStatePacket`.
- M06-003 integrates that bounded memory context into Controller Plan generation only, preserving current planning-request authority and the model-independent runtime boundary.
- Structured facts and deterministic filters come first; semantic/vector retrieval remains supplementary and later.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M06-003.md`: embed the canonical M06-002 memory context into the existing Controller planning request/application path, preserve the 64 KiB planning request bound and explicit precedence, and do not widen persistence authority or redesign other Controller capabilities.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and migration map.
