# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M05 — Planning and Lead unification

**Current task:** M05-002 — Make persisted Plan provenance Controller-compatible

**Last completed:** M05-001 — Add read-only Controller planning capability

**Blocked by:** Nothing; M05-002 was refocused after repository inspection exposed a legacy provenance constraint.

## Current decisions

- Local Controller from phase one.
- Initial model/runtime target: Qwen3 8B through llama.cpp/GGUF.
- Qwen/llama.cpp is an implementation choice, not a domain dependency; the runtime boundary must remain replaceable.
- `ProjectOperations` is the primary existing provider-independent observation seam.
- `OrcApp` is the existing canonical application/mutation seam.
- M02 is complete: bounded read-only Controller state/recommendation and reliable structured output are in place.
- M03 is complete: typed normal-action intents, deterministic legality, trusted one-shot authorization, fresh legality re-check, canonical execution, and supervised recommendation-to-intent bridge are in place.
- M04 is complete: bounded recovery facts/legality, Controller recovery choice, supervised recovery execution, validation-repair exhaustion migration, and semantic revision non-convergence migration are in place.
- M05-001 is complete: bounded read-only Controller planning returns a validated typed plan proposal through `LocalInferenceRuntime`; real Qwen strict-contract evaluation passed.
- Repository inspection for the original M05-002 scope found that persisted Plans universally require non-null Lead-decision and Planner-run foreign keys, and storage/review validation assumes that legacy lineage.
- Those fields are legacy workflow provenance, not intrinsic Plan invariants. Controller-origin Plans must be representable without fabricated or consumed Lead/Planner state.
- M05-002 now migrates durable Plan provenance to an explicit source-neutral representation while preserving strict legacy-origin validation and existing Plan status/version/parent/application semantics.
- Authorized Controller Plan persistence moves to the next task after M05-002 establishes truthful provenance.
- Deterministic validation, lifecycle legality, authorization, persistence, review/application state, and mutation remain kernel-owned.
- Existing Lead/Planner compatibility machinery remains until later M05 tasks migrate its judgment/routing safely.
- Memory is explicit Orc data, separate from model weights.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M05-002.md`: make persisted Plan provenance explicitly distinguish legacy Planner-origin from Controller-origin Plans. Preserve real legacy Lead/Planner lineage and validation; do not fabricate IDs, persist a production Controller proposal, create/apply tasks, approve plans, or remove legacy machinery.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and Lead/Planner migration map.
