# Orc Next Roadmap

The roadmap defines direction, not a frozen implementation plan. Only the current and next milestone should be decomposed deeply.

## M00 — Architecture and repository mapping — COMPLETE

Mapped the repository against the Controller/kernel target and established canonical application/observation seams.

## M01 — Native model runtime — COMPLETE

Model-independent local runtime with llama.cpp/GGUF Qwen3 8B integration.

## M02 — Read-only Controller — COMPLETE

Bounded project/task state and structured read-only Controller recommendations.

## M03 — Typed Controller tools — COMPLETE

Typed intents, deterministic legality, explicit authorization, canonical execution.

## M04 — Recovery intelligence — COMPLETE

Controller recovery judgment over deterministic failure/recovery facts.

## M05 — Planning and Lead unification — COMPLETE

Controller planning/intake/Plan review/revision with deterministic persistence and workflow gates.

## M06 — Persistent memory — COMPLETE

Typed User/Project/Episodic/Experience persistence, deterministic bounded Controller memory context, capability-local integration, and supervised capture/maintenance. Memory remains separate from model weights.

## M07 — Supervised autonomy — COMPLETE

Finite routine grants and supervised continuation, including bounded memory capture/maintenance and target selection, without bypassing deterministic mutation/authorization boundaries.

## M08 — Experience dataset — COMPLETE

M08 established the distinct canonical curated Controller experience dataset required by D-006. M08-001 established typed verified examples; M08-002 through M08-010 added capability-local curation across all nine inference-backed judgment boundaries; M08-011 added deterministic complete-dataset inventory. No automatic harvesting, runtime-memory coupling, embeddings, provider fallback, provider token hard cap, trainer-specific export, or continuous weight mutation.

## M09 — Controller specialization — CURRENT

Fine-tune/evaluate the local Controller model. A new model becomes default only when evaluation demonstrates improvement without unacceptable regressions.

M09-001 completed the trainer-neutral Active experience snapshot at `481fe624777a03dc841ae1742dd5f9461e854fc7`.

M09-002 completed the full-surface nine-capability semantic evaluation suite at `6ceed5cda0627c82667664e4d7d54790207edbf9`.

M09-003 completed reproducible production-aligned baseline reporting at `552144121367b730d48dc0d10c808e99a0739852`. Its ignored real-model path uses `ORC_QWEN3_GGUF`; no measured Qwen score was asserted because the model was unavailable.

M09-004 completed the deterministic candidate-vs-baseline promotion gate at `8ddc47bc498d57d6729db2a85fcf1b3eeae2370f`. It requires fully comparable reports, strict global pass improvement, no execution/error increase, no capability pass-count regression, no baseline-pass regression, no newly introduced Parse/Validation/Runtime failure, and exact accounting. Typed transitions, signed deltas, and ordered reasons are validated. The gate is read-only and model-independent.

M09-005 investigates native/offline specialization backends and the smallest justified training/export boundary. It must verify upstream support, Qwen3/GGUF/adapter compatibility, actual hardware/model/data readiness, and preserve canonical dataset and evaluation authority. No trainer is selected by assumption and no training occurs in this investigation.

Later M09 work should use the investigation to define narrow dataset preparation and controlled candidate training/evaluation tasks. A default-model change is considered only after actual comparable evidence satisfies the fixed gate and explicit operator authorization.

## M10 — Interface integration

Expose the mature Controller consistently through CLI, TUI and GUI using shared core APIs.
