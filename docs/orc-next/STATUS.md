# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M04 — Recovery intelligence

**Current task:** M04-001 — Expose bounded recovery facts and legal recovery operations (Done)

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
- Recovery follows the same architecture boundary: kernel exposes canonical facts and legal recovery operations; Controller judgment later chooses among them.
- Recovery legality must preserve valid review/revision lineage and keep infrastructure, dependency, and economy/agent constraints distinct from semantic failure.
- M04-001 is complete and remains observation/legality only: no Controller recovery choice, execution, retry loop, or broad policy removal.
- Memory is explicit Orc data, separate from model weights.
- Lead/Planner reasoning will move into Controller after the Controller foundation is proven; durable useful Plan/decision data is preserved.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Define the later M04-002 recovery-choice policy against the bounded observation
and legal-operation seam. Keep recovery choice, retry/repair value, dependency
resolution, escalation, and unusual infrastructure handling out of the kernel
until that policy is explicitly scoped.

Inventory current canonical recovery seams first. Then expose a bounded recovery observation and a small repository-grounded set of legal recovery operations. The deterministic kernel answers what is legal; it must not rank, select, or execute recovery choices.

Required distinctions include blocked revision with valid REVISE lineage, infrastructure failure, dependency blocking, no-eligible-agent/economy exhaustion, and abnormal states where no safe automatic recovery operation exists.

Keep inspection side-effect free. Do not execute recovery, mint Controller authorization, add continuation/retry loops, migrate planning/Lead, add memory, change interfaces, or modify the production Controller recommendation prompt/schema/parser.

M03-003 completion evidence:

- Luna + High source review: `PASS`;
- focused action tests: 16 passed;
- Controller evaluation tests: 9 passed;
- `cargo test --lib`: 309 passed;
- `cargo test --features llama-cpp --lib`: 315 passed;
- normal and feature clippy, fmt, and diff checks: passed;
- production recommendation semantics unchanged, so Qwen was not rerun;
- no M04 architectural blocker identified.

M04-001 completion evidence:

- bounded serializable `RecoveryObservation` and typed `RecoveryInspection` are
  derived from canonical `ProjectOperations` facts;
- included operations are inspected `Requeue`, `ResumeRevision`, and exact
  `AcknowledgeNonConvergence`; unsupported/speculative recovery actions expose
  no unsafe default;
- seven focused recovery tests passed, including all five required abnormal
  scenarios and side-effect-free bounded serialization;
- `cargo test --lib`: 316 passed; `cargo test --features llama-cpp --lib`: 322
  passed;
- relevant dispatch/revision regression tests: 87 passed; economy/lifecycle:
  12 passed; app API: 26 passed; operations: 8 passed; queue: 25 passed;
- normal and feature clippy, fmt, and diff checks passed;
- Controller recommendation prompt/schema/parser semantics were unchanged, so
  Qwen was not rerun;
- no M04-002 architectural blocker identified.

See `M00-REPOSITORY-MAP.md` for the repository-grounded recovery fact-versus-judgment classification.
