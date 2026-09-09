# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M10 — Interface integration

**Current task:** M10-002 — Add TUI run history and worktree/diff inspection

**Last completed:** M10-001 — Complete the TUI normal task lifecycle vertical slice

**Blocked work:** M09-007 remains planned and blocked on genuine source-data availability. Training additionally requires approved model artifacts, usable GPU capacity, dependency locking, trainer qualification, external transform review, conversion validation, and a real baseline.

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M00–M08 are complete. M08 provides the distinct canonical curated experience dataset across all nine inference-backed Controller capabilities.
- M09-001 completed at `481fe624777a03dc841ae1742dd5f9461e854fc7`: deterministic trainer-neutral Active experience snapshot, full canonical records, fail-closed validation, zero writes.
- M09-002 completed at `6ceed5cda0627c82667664e4d7d54790207edbf9`: versioned deterministic nine-capability semantic evaluation suite with production builder/validator reuse and exact aggregates.
- M09-003 completed at `552144121367b730d48dc0d10c808e99a0739852`: reproducible full-surface baseline reporting, production-aligned llama.cpp execution, bounded typed Parse/Validation/Runtime evidence. No actual Qwen score was recorded because `ORC_QWEN3_GGUF` was unavailable.
- M09-004 completed at `8ddc47bc498d57d6729db2a85fcf1b3eeae2370f`: versioned deterministic candidate comparison, exact typed outcome transitions, signed deltas, fail-closed comparability, and fixed strict promotion rules.
- M09-005 completed at `52a37429c8438e626106fd25fa23363df7496109`: repository/upstream investigation. Backend selection remains deferred; isolated ms-swift Qwen3-8B LoRA is only the conditional first qualification path, followed by merged HF → GGUF export and fixed evaluation.
- M09-006 completed at `8481d1874796a520df3e29fd4993c122b94b0ae0`: supported registry provisioning and canonical measurement. Inventory schema 1: total 0, Active 0, Retired 0. Snapshot schema 1: count 0, empty IDs/order/examples. Observed coverage 0/9; lifecycle, outcome, and verification-basis counts are zero. The earlier missing-table state is superseded by a measured empty source.
- M09-007 remains blocked on genuine canonical source-data availability. No synthetic examples, speculative transform execution, backend selection, training, or promotion is authorized.
- M10 interface work may proceed independently where it does not depend on specialization. `M10-interface-audit.md` established the repository-grounded gap ordering.
- M10-001 completed at `8106d75695da31dc5212b236eeeca60040e168b8`: the Rust TUI now covers normal task creation, canonical queue/detail inspection, non-blocking dispatch/review/revision/accept actions, manual execution refresh, validation/review evidence, previous-agent revision continuity, explicit acceptance, and illegal-action handling through existing application/read boundaries.
- M10-001 required no shared application API addition and introduced no duplicate lifecycle/scheduler/validation/revision/recovery logic, M09 change, Controller inference, or automatic experience harvesting.
- M10-002 is Planned. It is the next audited TUI adapter slice and is read-oriented: persisted run history/detail plus validated worktree and diff/change inspection through existing canonical run/read/Git/review boundaries.
- M10-002 deliberately excludes reject/cancel/requeue/recovery execution, dependency mutation, explicit agent selection, Controller actions, M08 curation, and general task metadata editing so those remain independently reviewable follow-ons.
- General post-creation task metadata editing remains a separate shared API seam identified by the M10 audit.
- D-006 requires controlled learning and evaluation; training completion alone cannot justify promotion. M09-004 fixes acceptance semantics before candidate results exist.
- D-007 keeps Orc runtime Rust/native, permits C/C++ inference/performance components, and treats training/export separately. A Python/PyTorch training/export environment may be isolated from Orc runtime but is not selected or added as a runtime dependency.
- No trainer/backend has been selected for execution, no candidate trained, no weights modified, and no model promoted.
- No embeddings, provider fallback, provider token hard cap, automatic weight mutation, or Python runtime dependency is introduced.

## Immediate next action

Implement M10-002 using existing canonical run/read, validated worktree, and Git/review evidence boundaries. Keep the task read-oriented and do not add lifecycle/recovery mutation or unsafe filesystem access. M09-007 remains blocked until supervised M08 curation produces genuine source data and fresh M08-011/M09-001 measurement confirms it.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
