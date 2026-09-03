# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M05 — Planning and Lead unification

**Current task:** M05-003 — Persist Controller Plan proposals through explicit authorization

**Last completed:** M05-002 — Make persisted Plan provenance Controller-compatible

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
- M05-002 is complete: persisted Plan provenance explicitly distinguishes legacy Planner origin from Controller origin; legacy lineage remains strict and truthful, while Controller-origin Proposed Plans require no fabricated Lead/Planner IDs.
- `Database::store_controller_plan` is the canonical deterministic storage seam for Controller-origin Proposed Plans; production Controller authorization/persistence has not yet been connected.
- M05-003 connects validated Controller plan proposals to that storage seam through trusted one-shot authorization and fresh deterministic validation only.
- Deterministic validation, lifecycle legality, authorization, persistence, review/application state, and mutation remain kernel-owned.
- Existing Lead/Planner compatibility machinery remains until later M05 tasks migrate its judgment/routing safely.
- Memory is explicit Orc data, separate from model weights.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M05-003.md`: authorize and persist a validated Controller plan proposal as exactly one canonical Controller-origin `Proposed` Plan through the M05-002 storage seam. Do not review, revise, approve, apply, create tasks, consume Lead state, invoke Planner, or remove legacy machinery.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and Lead/Planner migration map.
