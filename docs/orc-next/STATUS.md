# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M09 — Controller specialization

**Current task:** M09-004 — Define deterministic Controller candidate promotion gate

**Last completed:** M09-003 — Capture reproducible full-surface Controller baseline

**Blocked by:** Nothing

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M08 is complete: the canonical curated Controller experience dataset remains distinct from runtime `MemoryKind::Experience` and has typed persistence/lifecycle, capability-local curation for every current inference-backed Controller judgment boundary, and deterministic complete-dataset inventory.
- M09-001 is complete at `481fe624777a03dc841ae1742dd5f9461e854fc7`, adding a versioned trainer-neutral Active-dataset snapshot with exact canonical M08 records, ascending identity ordering, fail-closed validation of every persisted row before lifecycle filtering, deterministic serialization, and zero-write read behavior.
- M09-002 is complete at `6ceed5cda0627c82667664e4d7d54790207edbf9`, adding specialization-evaluation schema version 1 and one deterministic model-independent typed evaluation suite across all nine current inference-backed Controller capabilities with production builder/validator reuse, explicit semantic fixtures, non-aborting failure recording, and exact global/per-capability accounting.
- M09-003 is complete at `552144121367b730d48dc0d10c808e99a0739852`, adding a versioned typed full-surface baseline report over the exact M09-002 suite, production-aligned llama.cpp execution through `ORC_QWEN3_GGUF`, privacy-safe model identity, runtime/request evidence, deterministic aggregates, and distinct bounded Parse/Validation/Runtime evidence without production inference changes.
- The implementation/review environments did not provide `ORC_QWEN3_GGUF`, so no measured Qwen score has been canonically asserted. M09-003 remains code-complete and no score was invented.
- M09 specialization remains controlled under D-006: training completion alone can never justify model promotion; baseline and candidate evaluation evidence are required.
- M09-004 fixes deterministic candidate-vs-baseline promotion semantics before any candidate training result exists, preventing post-hoc acceptance-rule tuning.
- M09-004 compares only fully comparable M09-003 reports and requires strict global pass improvement, no capability pass-count regression, no regression of a baseline-passing scenario, and no newly introduced Parse/Validation/Runtime failure under the canonical failure taxonomy.
- M09-004 remains model-independent and read-only: no inference, training, trainer/backend selection, dataset transformation, persistence, or default-model mutation belongs in the comparison gate.
- Trainer/backend investigation, trainer-specific transformation, controlled training, candidate real-model evaluation, and any eventual default-model change remain later M09 work after the gate is fixed.
- No embeddings, model-specific core behavior, provider fallback, provider token hard cap, automatic weight mutation, or Python runtime dependency is introduced.

## Immediate next action

Implement `tasks/M09-004.md`: add a deterministic typed candidate-vs-baseline comparator and promotion gate over validated M09-003 reports, fixing strict no-regression acceptance semantics before candidate training begins.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
