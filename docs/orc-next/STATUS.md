# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M09 — Controller specialization

**Current task:** M09-005 — Investigate native/offline Controller specialization backend

**Last completed:** M09-004 — Define deterministic Controller candidate promotion gate

**Blocked by:** Nothing for investigation; actual training remains dependent on backend, hardware, model, and dataset readiness.

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M00–M08 are complete. M08 provides the distinct canonical curated experience dataset across all nine inference-backed Controller capabilities.
- M09-001 completed at `481fe624777a03dc841ae1742dd5f9461e854fc7`: deterministic trainer-neutral Active experience snapshot, full canonical records, fail-closed validation, zero writes.
- M09-002 completed at `6ceed5cda0627c82667664e4d7d54790207edbf9`: versioned deterministic nine-capability semantic evaluation suite with production builder/validator reuse and exact aggregates.
- M09-003 completed at `552144121367b730d48dc0d10c808e99a0739852`: reproducible full-surface baseline reporting, production-aligned llama.cpp execution, bounded typed Parse/Validation/Runtime evidence. No actual Qwen score was recorded because `ORC_QWEN3_GGUF` was unavailable.
- M09-004 completed at `8ddc47bc498d57d6729db2a85fcf1b3eeae2370f`: versioned deterministic candidate comparison, exact typed outcome transitions, signed deltas, fail-closed comparability, and fixed strict promotion rules. Independent PASS followed correction of coarse transition evidence. No inference or model behavior changed.
- D-006 requires controlled learning and evaluation; training completion alone cannot justify promotion. M09-004 fixes acceptance semantics before candidate results exist.
- D-007 keeps Orc runtime Rust/native, permits C/C++ inference/performance components, and treats training/export separately. Native options must be investigated before backend selection.
- No trainer/backend has been selected, no candidate trained, no weights modified, and no model promoted.
- No embeddings, provider fallback, provider token hard cap, automatic weight mutation, or Python runtime dependency is introduced.

## Immediate next action

Execute `tasks/M09-005.md`: investigate verified native/offline specialization options, repository compatibility, model/adapter formats, hardware and dataset readiness, and recommend the smallest justified training/export boundary. This is a documentation/evidence task, not training implementation.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
