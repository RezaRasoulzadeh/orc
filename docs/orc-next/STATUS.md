# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M06 — Persistent memory

**Current task:** M06-006 — Integrate bounded memory into Controller workflow intake

**Last completed:** M06-005 — Integrate bounded memory into normal Controller task recommendation

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
- M06-003 is complete: Controller Plan generation receives typed bounded memory through a capability-local planning input.
- M06-004 is complete: recovery recommendation receives typed bounded memory while exact current legal operations remain the only source of actionability.
- M06-005 is complete: normal task recommendation receives typed bounded memory while `ControllerStatePacket` remains the authoritative current-facts projection and M03 deterministic action boundaries remain unchanged.
- M06-006 integrates bounded memory into Controller workflow intake while preserving the canonical intake request, exactly three intake outcomes, and downstream workflow kernel authority.
- Structured facts and deterministic filters come first; semantic/vector retrieval remains supplementary and later.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M06-006.md`: integrate canonical bounded memory into `ControllerIntakeRequest` / `ControllerIntakeBuilder::classify` through `OrcApp::propose_controller_intake`, preserving current intake facts, the three-outcome schema, workflow routing, persistence, and application semantics.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and migration map.
