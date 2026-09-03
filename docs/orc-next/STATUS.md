# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M05 — Planning and Lead unification

**Current task:** M05 follow-up — migrate the next supervised legacy planning/routing seam

**Last completed:** M05-007 — Persist Controller Plan revisions through explicit authorization

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
- M05-001 through M05-003 provide bounded Controller Plan generation and explicitly authorized Controller-origin Plan persistence with truthful provenance.
- M05-004 and M05-005 provide bounded Controller Plan review judgment and explicitly authorized durable Controller-origin Plan review/status persistence.
- M05-006 is complete: Controller revision generation requires a current Controller-origin `RevisionRequested` Plan and its latest unsuperseded Controller-origin `RevisePlan` review; generation is read-only and Qwen strict/semantic evaluation passed 1/1.
- M05-007 connects validated Controller revisions to atomic canonical Plan version/parent/review supersession through trusted one-shot authorization.
- Deterministic validation, lifecycle legality, authorization, persistence, review/application state, and mutation remain kernel-owned.
- Existing Lead/Planner compatibility machinery remains until later M05 tasks migrate its judgment/routing safely.
- Memory is explicit Orc data, separate from model weights.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Select the next separately scoped supervised migration of the remaining legacy Lead/Planner routing boundary. Preserve plan application gates and do not remove compatibility machinery until its replacement is validated.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and Lead/Planner migration map.
