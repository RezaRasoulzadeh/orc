# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M09 — Controller specialization

**Current task:** M09-006 — Provision and measure the canonical specialization source

**Last completed:** M09-005 — Investigate native/offline Controller specialization backend

**Blocked by:** Nothing for canonical-source measurement. Actual training remains blocked on measured canonical data, approved model artifacts, usable GPU capacity, dependency locking, trainer qualification, external transform review, conversion validation, and a real baseline.

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M00–M08 are complete. M08 provides the distinct canonical curated experience dataset across all nine inference-backed Controller capabilities.
- M09-001 completed at `481fe624777a03dc841ae1742dd5f9461e854fc7`: deterministic trainer-neutral Active experience snapshot, full canonical records, fail-closed validation, zero writes.
- M09-002 completed at `6ceed5cda0627c82667664e4d7d54790207edbf9`: versioned deterministic nine-capability semantic evaluation suite with production builder/validator reuse and exact aggregates.
- M09-003 completed at `552144121367b730d48dc0d10c808e99a0739852`: reproducible full-surface baseline reporting, production-aligned llama.cpp execution, bounded typed Parse/Validation/Runtime evidence. No actual Qwen score was recorded because `ORC_QWEN3_GGUF` was unavailable.
- M09-004 completed at `8ddc47bc498d57d6729db2a85fcf1b3eeae2370f`: versioned deterministic candidate comparison, exact typed outcome transitions, signed deltas, fail-closed comparability, and fixed strict promotion rules. Independent PASS followed correction of coarse transition evidence. No inference or model behavior changed.
- M09-005 completed at `52a37429c8438e626106fd25fa23363df7496109`: repository/upstream specialization investigation. Backend selection remains deferred; isolated ms-swift Qwen3-8B LoRA is only the conditional first qualification path, followed by merged HF → GGUF export and the fixed M09-003/M09-004 evaluation flow.
- M09-005 measured insufficient local execution readiness: no usable GPU, no local Qwen/GGUF artifact, no recorded real baseline, and the configured registry lacked the canonical experience table, so dataset count was unknown rather than zero.
- D-006 requires controlled learning and evaluation; training completion alone cannot justify promotion. M09-004 fixes acceptance semantics before candidate results exist.
- D-007 keeps Orc runtime Rust/native, permits C/C++ inference/performance components, and treats training/export separately. A Python/PyTorch training/export environment may be isolated from Orc runtime but is not selected or added as a runtime dependency.
- No trainer/backend has been selected for execution, no candidate trained, no weights modified, and no model promoted.
- No embeddings, provider fallback, provider token hard cap, automatic weight mutation, or Python runtime dependency is introduced.

## Immediate next action

Execute `tasks/M09-006.md`: resolve and provision the intended global registry through existing Orc authority, then capture factual M08-011 inventory and M09-001 Active snapshot evidence, including exact nine-capability coverage. Do not export, transform, train, or infer readiness.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
