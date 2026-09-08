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

## M09 — Controller specialization — BLOCKED / DEFERRED

Fine-tune/evaluate the local Controller model. A new model becomes default only when evaluation demonstrates improvement without unacceptable regressions.

M09-001 completed the trainer-neutral Active experience snapshot at `481fe624777a03dc841ae1742dd5f9461e854fc7`.

M09-002 completed the full-surface nine-capability semantic evaluation suite at `6ceed5cda0627c82667664e4d7d54790207edbf9`.

M09-003 completed reproducible production-aligned baseline reporting at `552144121367b730d48dc0d10c808e99a0739852`. Its ignored real-model path uses `ORC_QWEN3_GGUF`; no measured Qwen score was asserted because the model was unavailable.

M09-004 completed the deterministic candidate-vs-baseline promotion gate at `8ddc47bc498d57d6729db2a85fcf1b3eeae2370f`. It requires fully comparable reports, strict global pass improvement, no execution/error increase, no capability pass-count regression, no baseline-pass regression, no newly introduced Parse/Validation/Runtime failure, and exact accounting. Typed transitions, signed deltas, and ordered reasons are validated. The gate is read-only and model-independent.

M09-005 completed the native/offline specialization investigation at `52a37429c8438e626106fd25fa23363df7496109`. Backend selection was deliberately deferred. Isolated ms-swift Qwen3-8B LoRA is the conditional first qualification path, followed by merged Hugging Face → GGUF conversion/quantization and existing M09-003/M09-004 evaluation. Native llama.cpp training remains unsuitable for first selection based on current upstream WIP/limited evidence. No trainer or Python dependency was added to Orc runtime.

M09-006 completed canonical source provisioning and measurement at `8481d1874796a520df3e29fd4993c122b94b0ae0`. The supported Orc initialization path provisioned the missing experience schema. M08-011 inventory schema 1 measured total 0, Active 0, Retired 0; M09-001 snapshot schema 1 measured count 0 and empty examples/IDs/order. Observed capability coverage is 0/9 and categorical distributions are zero. The earlier unknown count is superseded by this measured empty source. No canonical records or production behavior changed.

M09-007 is planned but blocked on genuine canonical source-data availability. It will specify a versioned external snapshot-to-trainer transformation and immutable manifest only after existing supervised M08 curation and fresh inventory/snapshot evidence establish actual source records. No synthetic data, assumed readiness threshold, or speculative trainer-specific implementation is authorized.

Later M09 work remains independently reviewable: qualify one pinned Qwen3-8B LoRA toolchain on supplied hardware/model inputs; package and validate a candidate GGUF with full lineage; then execute M09-003/M09-004 candidate evaluation. A default-model change is considered only after actual comparable evidence satisfies the fixed gate and explicit operator authorization. No backend is selected for execution.

M09 specialization is therefore blocked/deferred while interface work that does not depend on training proceeds. This does not mark M09 complete and does not relax any source, evaluation, or promotion requirement.

## M10 — Interface integration — CURRENT

Expose the mature Controller consistently through CLI, TUI and GUI using shared core APIs.

The repository-grounded `M10-interface-audit.md` found that the CLI has the broadest complete workflow coverage, the TUI is operationally narrow, and the Tauri bridge exposes more canonical capability than the Vue adapter currently wires. Existing application/read-model APIs are sufficient for the first interface slice; general post-creation task metadata editing is the only clear shared application API gap identified by the audit.

M10-001 is the first planned implementation task: complete a normal task lifecycle in the TUI using existing canonical APIs. Its vertical slice covers task creation, queue/detail inspection, dispatch, execution refresh, validation/review evidence, revision with operator feedback through the canonical revision path, and explicit acceptance. It must not duplicate lifecycle/scheduler/validation/review logic, introduce Controller inference or automatic experience harvesting, or change M09 specialization state.

Follow-on M10 work should remain independently reviewable and proceed from actual operator gaps: TUI run/history and worktree/diff/recovery controls; typed Controller authorization adapter exposure; supervised M08 curation surfaces; one shared general task-metadata update seam; and remaining desktop governance/economy/Plan/Lead parity gaps. Interface work must continue to preserve canonical Rust ownership of facts, legality, authorization, persistence, validation, and workflow transitions.
