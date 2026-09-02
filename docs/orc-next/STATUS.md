# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M05 — Planning and Lead unification

**Current task:** M05-001 — Add read-only Controller planning capability

**Last completed:** M04-005 — Route semantic revision non-convergence into supervised Controller recovery

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
- Deterministic validation, review/revision lineage, lifecycle legality, agent eligibility/quota, economy observations, authorization, and mutation remain kernel-owned.
- A recommendation or prior Allowed result is never authorization or a durable legality grant.
- Model-owned recommendation/intent cannot carry or manufacture authorization.
- Memory is explicit Orc data, separate from model weights.
- M05 moves planning and Lead-like judgment into Controller while preserving useful durable Plan/approval data and removing obsolete duplicated role/handoff machinery incrementally.
- M05-001 starts with a read-only Controller planning seam; current Lead/Planner routing and durable plan persistence remain unchanged until later migration tasks.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M05-001.md`: add bounded read-only Controller planning through `LocalInferenceRuntime`, preserving existing `PlanningRequest`/`PlanResponse` validation and durable Plan/Lead machinery. No plan/task/workflow mutation or legacy Planner/Lead removal in this task.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and Lead/Planner migration map.
