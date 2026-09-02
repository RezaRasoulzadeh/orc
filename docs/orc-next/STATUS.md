# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M04 — Recovery intelligence

**Current task:** M04-002 — Add read-only Controller recovery choice

**Last completed:** M04-001 — Expose bounded recovery facts and legal recovery operations

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
- Recovery follows the same architecture boundary: kernel exposes canonical facts and legal recovery operations; Controller judgment chooses among them.
- M04-001 is complete and source-reviewed `PASS`: bounded recovery facts and deterministic legality are exposed through `RecoveryObservation` / `RecoveryInspection` with repository-grounded `Requeue`, `ResumeRevision`, and `AcknowledgeNonConvergence` operations.
- Requeue inspection and mutation share canonical legality; recovery classification scans complete candidate/execution sets before bounded projection.
- Recovery legality preserves valid review/revision lineage and keeps infrastructure, dependency, and economy/agent constraints distinct from semantic failure.
- M04-002 adds read-only Controller recovery choice only; execution/authorization remains out of scope.
- Memory is explicit Orc data, separate from model weights.
- Lead/Planner reasoning will move into Controller after the Controller foundation is proven; durable useful Plan/decision data is preserved.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement M04-002 against `tasks/M04-002.md`: feed bounded M04-001 recovery observation and legal-operation facts through `LocalInferenceRuntime`, obtain a structured `RecoveryRecommendation`, and deterministically accept a recovery choice only when that exact operation is legal in the supplied inspection. `OperatorDecision` remains the non-mutating fallback.

M04-002 must not execute recovery, mint authorization, add continuation/retry loops, add new recovery operations, weaken validation/revision lineage, migrate planning/Lead, add memory, change interfaces, replace the model, add GPU work, or introduce Python.

Because M04-002 introduces new production recovery prompt/schema semantics, deterministic validation must pass before running the real local Qwen recovery evaluation. Report strict structured-contract compliance and semantic decision quality separately.

M04-001 final completion evidence:

- Luna + High source review: `PASS` after three blockers were corrected;
- shared canonical requeue legality drives inspection and mutation;
- economy classification examines the complete candidate set before bounded projection;
- latest relevant failed/cancelled execution is selected from complete history before bounded projection;
- focused recovery tests: 9 passed;
- `cargo test --lib`: 318 passed;
- `cargo test --features llama-cpp --lib`: 324 passed;
- dispatch/revision: 87 passed; economy/lifecycle: 12 passed; app API: 26 passed; operations: 8 passed; queue: 25 passed;
- normal and feature clippy, fmt, and diff checks passed;
- production Controller semantics were unchanged, so Qwen was not rerun;
- no M04-002 architectural blocker identified.

See `M00-REPOSITORY-MAP.md` for the repository-grounded recovery fact-versus-judgment classification.
