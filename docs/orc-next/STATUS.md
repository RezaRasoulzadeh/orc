# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M04 — Recovery intelligence

**Current task:** M04-003 — Execute explicitly authorized recovery recommendations

**Last completed:** M04-002 — Add read-only Controller recovery choice

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one.
- Initial model/runtime target: Qwen3 8B through llama.cpp/GGUF.
- Qwen/llama.cpp is an implementation choice, not a domain dependency; the runtime boundary must remain replaceable.
- `ProjectOperations` is the primary existing provider-independent observation seam.
- `OrcApp` is the existing canonical application/mutation seam.
- M02 is complete: the final seven-scenario Qwen3 evaluation achieved `7/7` semantic decisions and `7/7` strict structured-contract compliance.
- M03 is complete: typed normal-action intents, deterministic legality inspection, opaque one-shot trusted authorization, fresh pre-mutation legality re-check, canonical execution, and the supervised recommendation-to-intent bridge are in place.
- A legality inspection or Controller recommendation is not authorization and is not a durable grant.
- Model-owned recommendation/intent must never carry or manufacture its own authorization/confirmation.
- M04-001 is complete and source-reviewed `PASS`: bounded recovery facts and deterministic legality are exposed through `RecoveryObservation` / `RecoveryInspection` with repository-grounded `Requeue`, `ResumeRevision`, and `AcknowledgeNonConvergence` operations.
- Requeue inspection and mutation share canonical legality; recovery classification scans complete candidate/execution sets before bounded projection.
- M04-002 is complete and source-reviewed `PASS`: bounded recovery inference chooses only among inspected operations or `OperatorDecision`, with exact Allowed-membership validation and no mutation/authorization.
- Real Qwen M04-002 recovery evaluation achieved 7/7 strict structured-contract compliance and 7/7 semantic decision quality.
- M04-003 completes the supervised recovery execution boundary: trusted one-shot authorization, fresh legality inspection, then existing canonical recovery mutation.
- Recovery legality preserves valid review/revision lineage and keeps infrastructure, dependency, and economy/agent constraints distinct from semantic failure.
- Memory is explicit Orc data, separate from model weights.
- Lead/Planner reasoning will move into Controller after the Controller foundation is proven; durable useful Plan/decision data is preserved.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement M04-003 against `tasks/M04-003.md`. Convert only an M04-002 actionable recovery recommendation into a bounded recovery execution intent. Trusted Orc/application code explicitly authorizes exactly one task + operation. Immediately before mutation, freshly inspect M04-001 legality and require the exact operation to remain Allowed, then delegate to the existing canonical mutation.

Support only `Requeue`, `ResumeRevision`, and `AcknowledgeNonConvergence`. `OperatorDecision` and rejected recommendations remain non-executable. Resume-revision worker/validation configuration remains trusted application-owned execution context, not model data.

Do not add autonomous recovery loops, new recovery operations, prompt/schema changes, planning/Lead migration, memory, UI work, model changes, GPU work, or Python. Real Qwen does not need rerunning unless production recovery inference semantics unexpectedly change.

M04-002 final completion evidence:

- focused recovery recommendation tests: 8 passed;
- M04-001 recovery tests: 9 passed;
- Controller evaluation tests: 9 passed;
- `cargo test --lib`: 326 passed;
- `cargo test --features llama-cpp --lib`: 332 passed;
- normal and feature clippy, fmt, and diff checks passed;
- real Qwen recovery evaluation: strict 7/7, semantic 7/7;
- Luna + High source review: `PASS`;
- no M04-003 architectural blocker identified.

See `M00-REPOSITORY-MAP.md` for the repository-grounded recovery fact-versus-judgment classification.
