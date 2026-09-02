# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M04 — Recovery intelligence

**Current task:** M04-004 — Route validation-repair exhaustion into supervised Controller recovery

**Last completed:** M04-003 — Execute explicitly authorized recovery recommendations

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
- A recommendation or prior Allowed result is never authorization or a durable legality grant.
- Model-owned recommendation/intent cannot carry or manufacture authorization.
- Recovery legality preserves valid review/revision lineage and keeps infrastructure, dependency, and economy/agent constraints distinct from semantic failure.
- M04-004 begins policy migration narrowly with validation-repair exhaustion; deterministic validation remains kernel-owned while post-exhaustion recovery judgment moves to the Controller boundary.
- Memory is explicit Orc data, separate from model weights.
- Lead/Planner reasoning will move into Controller after the Controller foundation is proven; durable useful Plan/decision data is preserved.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement M04-004 against `tasks/M04-004.md`. Inspect the real implementation/revision validation-repair exhaustion paths first. Preserve deterministic validation, run evidence, and revision lineage, but route the post-exhaustion abnormal state into the existing M04 recovery observation/recommendation/supervised-execution boundary instead of encoding another recovery-choice heuristic.

Keep the task narrow: do not broadly rewrite scheduler/economy escalation or remove bounded repair attempts themselves. Infrastructure failure must remain distinct. Economy/quota exhaustion must remain a constraint/fact and must not destroy valid revision actionability. If the current three M04 recovery operations cannot represent the real repository path, stop and report before adding an operation.

M04-003 final completion evidence:

- `RecoveryExecutionIntent` / proposal derived only from M04-002 actionable validation;
- opaque non-serializable one-shot `RecoveryActionAuthorization` bound to exact task + operation;
- trusted native `RecoveryExecutionContext`;
- fresh `OrcApp::inspect_recovery` immediately before mutation;
- canonical `OrcApp::requeue`, shared revision execution, and `OrcApp::unblock_non_convergence` delegation;
- focused recovery execution tests: 10 passed;
- `cargo test --lib`: 336 passed;
- `cargo test --features llama-cpp --lib`: 342 passed;
- normal/feature clippy, fmt, and diff checks passed;
- production recovery inference semantics unchanged, so Qwen was not rerun;
- Luna + High source review: `PASS`.

See `M00-REPOSITORY-MAP.md` for the repository-grounded validation/economy fact-versus-judgment classification.
