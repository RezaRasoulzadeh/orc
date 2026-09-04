# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M06 — Persistent memory

**Current task:** M06-001 — Establish typed persistent memory records and deterministic storage

**Last completed:** M05-009 — Replace Lead intake judgment with supervised Controller routing

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
- M06-001 establishes the canonical typed durable memory model and deterministic project/global persistence before any Controller retrieval or autonomous memory behavior.
- User/experience memory is cross-project global state; project/episodic memory is project-scoped. Working memory remains transient bounded context.
- Structured facts and deterministic filters come first; semantic/vector retrieval remains supplementary and later.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M06-001.md`: establish typed inspectable/correctable/removable memory records, project/global storage placement, transactional correction/removal, structured deterministic queries, migration compatibility, and cross-project isolation. Do not yet feed memory into Controller prompts or add semantic/vector retrieval.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and migration map.
