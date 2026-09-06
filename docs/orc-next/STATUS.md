# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M09 — Controller specialization

**Current task:** M09-002 — Establish full-surface Controller specialization evaluation suite

**Last completed:** M09-001 — Add deterministic trainer-neutral Controller dataset snapshot

**Blocked by:** Nothing

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M08 is complete: the canonical curated Controller experience dataset remains distinct from runtime `MemoryKind::Experience` and has typed persistence/lifecycle, capability-local curation for every current inference-backed Controller judgment boundary, and deterministic complete-dataset inventory.
- M09-001 is complete at `481fe624777a03dc841ae1742dd5f9461e854fc7`, adding a versioned trainer-neutral Active-dataset snapshot with exact canonical M08 records, ascending identity ordering, fail-closed validation of every persisted row before lifecycle filtering, deterministic serialization, and zero-write read behavior.
- M09 specialization must remain controlled under D-006: a candidate model cannot be promoted merely because training completed; evaluation evidence is required.
- The existing M02 Controller evaluation harness covers the earlier normal-recommendation surface, but it predates recovery, planning, workflow intake, Plan review/revision, memory capture, memory maintenance, and memory-selection capabilities.
- M09-002 therefore establishes one deterministic, model-independent evaluation suite covering all nine current inference-backed Controller capability identifiers before trainer/backend selection or model promotion policy.
- M09-002 must compare typed semantic results rather than prose, keep expectations explicit and independent of observed model output, preserve deterministic scenario identities/order and aggregate accounting, and remain read-only with fake-runtime coverage requiring no model file.
- M09-002 must not change production inference prompts/parsers/runtime settings, derive expected answers automatically from M09-001 training records, choose a training backend, define train/validation/test splitting, or set a promotion threshold.
- Fine-tuning/export transformation and the eventual candidate-vs-baseline promotion gate remain later M09 decisions after the full capability evaluation substrate exists.
- No embeddings, model-specific core behavior, provider fallback, provider token hard cap, automatic weight mutation, or Python runtime dependency is introduced.

## Immediate next action

Implement `tasks/M09-002.md`: a deterministic full-surface Controller specialization evaluation suite covering every current inference-backed capability through production-aligned typed semantic evaluation, without training or production inference changes.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
