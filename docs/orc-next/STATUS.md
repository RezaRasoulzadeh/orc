# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M06 — Persistent memory

**Current task:** Define the first narrow M06 persistent-memory task from the repository-grounded storage and Controller seams

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
- M05 Controller workflow recovery is exact, workflow-bound, restart-safe, and does not fabricate provider/Lead/Planner lineage.
- Memory is explicit Orc data, separate from model weights.
- M06 will introduce inspectable persistent memory with explicit scope/provenance/precedence before adding semantic/vector retrieval or learned consolidation.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Inspect existing project/global persistence, Controller state construction, project identity, and configuration seams, then define the smallest M06 task that establishes durable typed memory records and deterministic read/write semantics without yet adding vector retrieval, autonomous memory writes, or training behavior.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and migration map.
