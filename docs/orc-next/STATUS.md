# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M05 — Planning and Lead unification

**Current task:** M05-002 — Persist Controller plan proposals through explicit authorization

**Last completed:** M05-001 — Add read-only Controller planning capability

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
- M05-001 is complete: bounded read-only Controller planning now returns a validated typed plan proposal through `LocalInferenceRuntime`; real Qwen strict-contract evaluation passed.
- Deterministic validation, lifecycle legality, authorization, persistence, review/application state, and mutation remain kernel-owned.
- A Controller plan result is not authorization and cannot directly persist or apply itself.
- M05-002 adds only supervised persistence of a validated Controller plan as a canonical `Proposed` Plan. Existing plan review and approved-plan application remain unchanged.
- Existing Lead/Planner compatibility machinery remains until later M05 tasks migrate its judgment/routing safely.
- Memory is explicit Orc data, separate from model weights.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M05-002.md`: connect a validated M05-001 Controller plan result to canonical `Proposed` Plan persistence through an explicit trusted one-shot authorization boundary. Do not create tasks, approve/apply plans, fabricate Lead/Planner provenance, or remove legacy machinery.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and Lead/Planner migration map.
