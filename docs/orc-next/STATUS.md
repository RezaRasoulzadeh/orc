# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M06 — Persistent memory

**Current task:** M06-005 — Integrate bounded memory into normal Controller task recommendation

**Last completed:** M06-004 — Integrate bounded memory into Controller recovery

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
- M06-003 is complete: Controller Plan generation receives typed bounded memory through a capability-local planning input with explicit precedence and combined 64 KiB enforcement; real Qwen precedence evaluation passed.
- M06-004 is complete: recovery recommendation receives typed bounded memory through a capability-local recovery input; exact current legal operations remain the only source of actionability and real Qwen legality-precedence evaluation passed.
- M06-005 integrates the same bounded memory context into the original normal task recommendation seam while keeping `ControllerStatePacket` a distinct current-facts projection and all M03 deterministic action boundaries unchanged.
- Structured facts and deterministic filters come first; semantic/vector retrieval remains supplementary and later.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M06-005.md`: integrate canonical bounded memory into the existing normal Controller recommendation / `OrcApp::propose_controller_action` path without widening `ControllerStatePacket` or changing typed recommendation, legality, authorization, or execution semantics.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and migration map.
