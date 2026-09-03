# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M05 — Planning and Lead unification

**Current task:** M05-009 — Replace Lead intake judgment with supervised Controller routing

**Last completed:** M05-008 — Route supervised Plan workflow through Controller capabilities

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
- M05-001 through M05-007 provide Controller Plan generation/review/revision judgment plus truthful explicitly authorized persistence.
- M05-008 routes persisted supervised Plan generation/review/revision through those Controller boundaries with exact workflow-bound restart recovery, truthful provider-less outcomes, preserved approval gates, and legacy compatibility.
- M05-009 targets the remaining normal-workflow legacy semantic seam: Lead intake classification among DirectTasks, PlanRequired, and UserDecisionRequired.
- Deterministic validation, lifecycle legality, authorization, persistence, workflow routing, review/application state, and mutation remain kernel-owned.
- Legacy Lead/Planner compatibility machinery may remain after M05 if it is no longer semantically required by the normal supervised Controller workflow.
- Memory is explicit Orc data, separate from model weights.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement `tasks/M05-009.md`. If its acceptance criteria pass and the normal supervised Controller workflow has no remaining semantic dependency on legacy Lead/Planner, close M05 and proceed to M06 rather than creating cleanup tasks solely to delete compatibility code.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and Lead/Planner migration map.
