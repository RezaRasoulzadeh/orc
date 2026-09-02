# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M04 — Recovery intelligence

**Current task:** M04-005 — Route semantic revision non-convergence into supervised Controller recovery

**Last completed:** M04-004 — Route validation-repair exhaustion into supervised Controller recovery

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one.
- Initial model/runtime target: Qwen3 8B through llama.cpp/GGUF.
- Qwen/llama.cpp is an implementation choice, not a domain dependency; the runtime boundary must remain replaceable.
- `ProjectOperations` is the primary existing provider-independent observation seam.
- `OrcApp` is the existing canonical application/mutation seam.
- M02 is complete: Qwen3 evaluation achieved 7/7 semantic decisions and 7/7 strict structured-contract compliance.
- M03 is complete: typed normal-action intents, deterministic legality, trusted one-shot authorization, fresh legality re-check, canonical execution, and supervised recommendation-to-intent bridge are in place.
- M04-001 exposes bounded canonical recovery facts and legal `Requeue`, `ResumeRevision`, and `AcknowledgeNonConvergence` operations.
- M04-002 adds read-only recovery judgment validated against the exact M04-001 Allowed set; real Qwen recovery evaluation passed 7/7 strict and 7/7 semantic.
- M04-003 completes supervised recovery execution with opaque one-shot authorization, fresh M04-001 legality inspection, and canonical mutation delegation.
- M04-004 removes automatic economy escalation after validation-repair exhaustion while preserving deterministic validation, failed-run evidence, blocked state, infrastructure classification, and actionable revision lineage.
- A recommendation or prior Allowed result is never authorization or a durable legality grant.
- Model-owned recommendation/intent cannot carry or manufacture authorization.
- Recovery legality preserves valid review/revision lineage and keeps infrastructure, dependency, and economy/agent constraints distinct from semantic failure.
- M04-005 migrates semantic revision non-convergence next: repeated semantic failure remains a kernel fact; the decision to escalate/retry/stop belongs to Controller recovery.
- Memory is explicit Orc data, separate from model weights.
- Lead/Planner reasoning will move into Controller after the Controller foundation is proven; durable useful Plan/decision data is preserved.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement M04-005 against `tasks/M04-005.md`. Inspect the exact `SemanticRevisionNonConvergence` detection and escalation call first. Preserve semantic review/revision evidence and canonical factual detection, but remove only the automatic post-detection escalation judgment where it conflicts with the supervised M04 recovery boundary.

First-time REVISE, PASS/AcceptanceReady, validation-repair exhaustion, infrastructure failure, and exact `NonConvergenceReplanRequired` semantics must remain distinct. Do not add a new recovery operation or change recovery inference semantics without stopping and reporting first.

M04-004 final completion evidence:

- removed only the post-validation-repair-exhaustion economy escalation call;
- deterministic validation and bounded repair attempts unchanged;
- implementation exhaustion preserves failing validation/failed-run evidence without synthetic revision lineage;
- revision exhaustion preserves actionable REVISE lineage, allowing canonical `ResumeRevision` while generic requeue remains rejected;
- infrastructure failure remains distinct;
- economy/quota classification remains factual, with no automatic escalation request or `economy_escalation_exhausted` dead-end after repair exhaustion;
- focused dispatch/revision/economy tests: 99 passed;
- `cargo test --lib`: 336 passed;
- `cargo test --features llama-cpp --lib`: 342 passed;
- normal/feature clippy, fmt, and diff checks passed;
- Qwen not rerun because recovery inference semantics were unchanged;
- Luna + High source review: `PASS`.

See `M00-REPOSITORY-MAP.md` for the repository-grounded validation/economy fact-versus-judgment classification.
