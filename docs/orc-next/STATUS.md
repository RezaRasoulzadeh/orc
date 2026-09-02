# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M05 — Planning and Lead unification

**Current task:** Define first repository-grounded M05 migration task

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
- Migrated abnormal recovery paths no longer automatically invoke economy escalation; Controller chooses only among kernel-legal recovery operations under explicit authorization.
- A recommendation or prior Allowed result is never authorization or a durable legality grant.
- Model-owned recommendation/intent cannot carry or manufacture authorization.
- Memory is explicit Orc data, separate from model weights.
- M05 moves planning and Lead-like judgment into Controller while preserving useful durable Plan/approval data and removing obsolete duplicated role/handoff machinery incrementally.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Inspect the current planning and Lead implementation, persisted plan/decision data, CLI entry points, and application boundaries. Define the smallest M05 task that moves one concrete planning/Lead judgment path into the existing Controller architecture without deleting durable Plan/approval data or performing a broad rewrite.

M04-005 final completion evidence:

- deterministic repeated semantic-blocker detection preserved through `record_semantic_revision_non_convergence`;
- bounded `semantic_revision_non_convergence_detected` lifecycle evidence persisted;
- automatic semantic non-convergence economy escalation/exhaustion removed;
- review, revision contract/currentness, lineage, execution, and attempt evidence preserved;
- eligibility/quota/model-tier/invocation/usage remain canonical facts;
- M04-001 exposes canonical `ResumeRevision` where legal and rejects lineage-destroying generic requeue;
- exact non-convergence acknowledgement semantics unchanged;
- dispatch/revision 87, economy/lifecycle 12, lifecycle 22, operations 8, queue 25 tests passed;
- `cargo test --lib`: 336 passed;
- `cargo test --features llama-cpp --lib`: 342 passed;
- normal/feature clippy, fmt, and diff checks passed;
- Qwen not rerun because recovery inference semantics were unchanged;
- Luna + High source review: `PASS`.

See `M00-REPOSITORY-MAP.md` for the repository-grounded fact-versus-judgment classification and Lead/Planner migration map.
