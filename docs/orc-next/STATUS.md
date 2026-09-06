# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M09 — Controller specialization

**Current task:** M09-003 — Capture reproducible full-surface Controller baseline

**Last completed:** M09-002 — Establish full-surface Controller specialization evaluation suite

**Blocked by:** Nothing

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M08 is complete: the canonical curated Controller experience dataset remains distinct from runtime `MemoryKind::Experience` and has typed persistence/lifecycle, capability-local curation for every current inference-backed Controller judgment boundary, and deterministic complete-dataset inventory.
- M09-001 is complete at `481fe624777a03dc841ae1742dd5f9461e854fc7`, adding a versioned trainer-neutral Active-dataset snapshot with exact canonical M08 records, ascending identity ordering, fail-closed validation of every persisted row before lifecycle filtering, deterministic serialization, and zero-write read behavior.
- M09-002 is complete at `6ceed5cda0627c82667664e4d7d54790207edbf9`, adding specialization-evaluation schema version 1 and one deterministic model-independent typed evaluation suite across all nine current inference-backed Controller capabilities with production builder/validator reuse, explicit semantic fixtures, non-aborting failure recording, and exact global/per-capability accounting.
- M09-002 retains distinct semantic branches for task recommendation, recovery, workflow intake, Plan review, memory capture, memory maintenance, and memory selection while preserving representative planning and Plan revision coverage. Evaluation remains read-only and changes no production inference behavior.
- M09 specialization remains controlled under D-006: training completion alone can never justify model promotion; baseline and candidate evaluation evidence are required.
- The existing M02 `ORC_QWEN3_GGUF` smoke path proves native local-model execution but covers only the original recommendation corpus and adds evaluation-specific prompt instructions.
- M09-003 therefore captures a reproducible current-Qwen baseline by running the exact M09-002 full-surface suite through production-aligned capability paths, without changing prompts/runtime semantics to improve benchmark performance.
- M09-003 must preserve exact scenario authority and failure evidence, report stable suite/model/runtime identity plus per-scenario/global/per-capability results, remain read-only, and avoid a new database persistence boundary.
- Trainer/backend selection, trainer-specific transformation, dataset splitting/balancing/sampling, candidate-vs-baseline promotion thresholds, controlled training, and model-default changes remain later M09 decisions after baseline evidence exists.
- No embeddings, model-specific core behavior, provider fallback, provider token hard cap, automatic weight mutation, or Python runtime dependency is introduced.

## Immediate next action

Implement `tasks/M09-003.md`: add an ignored llama-cpp real-model baseline path over the exact M09-002 full-surface specialization suite, preserving production inference semantics and reproducible structured evidence without training or mutation.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
