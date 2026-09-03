# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M05 — Planning and Lead unification

**Current task:** M05-006 — Add read-only Controller Plan revision generation

**Last completed:** M05-005 — Persist Controller Plan review decisions through explicit authorization

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
- M05-001 is complete: bounded read-only Controller planning returns a validated typed plan proposal through `LocalInferenceRuntime`; real Qwen strict-contract evaluation passed.
- M05-002 is complete: persisted Plan provenance distinguishes legacy Planner and Controller origins without fabricated lineage.
- M05-003 is complete: validated Controller Plan proposals persist as Controller-origin `Proposed` Plans only through trusted one-shot authorization and fresh deterministic validation.
- M05-004 is complete: Plan review semantic judgment runs through the local Controller as a bounded read-only three-way decision; Qwen strict and semantic evaluation passed 3/3.
- M05-005 is complete: Plan review provenance distinguishes legacy Lead and Controller origins, and validated Controller reviews persist only through trusted one-shot authorization and canonical deterministic status transitions.
- M05-006 migrates semantic Plan revision generation from legacy Planner behavior into a bounded read-only Controller capability. Persistence/supersession remains a later explicit authorization seam.
- Deterministic validation, lifecycle legality, authorization, persistence, review/application state, and mutation remain kernel-owned.
- Existing Lead/Planner compatibility machinery remains until later M05 tasks migrate its judgment/routing safely.
- Memory is explicit Orc data, separate from model weights.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M05-006.md`: generate a validated revised `PlanResponse` from a current Controller-origin Plan and its latest actionable Controller-origin revise review. Reject stale/legacy/ineligible state before inference and perform no persistence, supersession, approval, application, task creation, or workflow continuation.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and Lead/Planner migration map.
