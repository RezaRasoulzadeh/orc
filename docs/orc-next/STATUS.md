# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M09 — Controller specialization

**Current task:** M09-001 — Add deterministic trainer-neutral Controller dataset snapshot

**Last completed:** M08-011 — Add deterministic Controller experience dataset inventory

**Blocked by:** Nothing

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M06 and M07 are complete.
- M08 is complete: the canonical curated Controller experience dataset remains distinct from runtime `MemoryKind::Experience` and now has typed persistence/lifecycle, capability-local curation for every current inference-backed Controller judgment boundary, and deterministic complete-dataset inventory.
- M08-010 is complete at `87db2f24151d3522394f7ed9a2a7042aedf66b50` with fixed capability `controller.memory_selection` and exact production selection validation preserved.
- M08-011 is complete at `6791f81e38cf9af6ac04d16d9a6d5998e3369337`, adding a read-only metadata-only inventory with exact global/per-capability lifecycle, outcome, and verification counts and fail-closed persisted metadata validation.
- Verification, accepted output, quality, correction, and provenance remain explicit trusted inputs. Downstream execution/mutation/workflow outcomes do not automatically label examples.
- M09 now begins specialization preparation. The first repository-grounded prerequisite is a deterministic trainer-neutral snapshot of the complete Active canonical M08 dataset.
- M09-001 must preserve exact canonical M08 example fields, exclude Retired examples, use stable code-owned ordering, fail closed on malformed persisted rows, and perform no writes or inference.
- M09-001 must not choose a training backend, translate examples into trainer-specific prompt/chat formats, split/balance/sample/rank/weight examples, infer readiness, or introduce Python into Orc runtime.
- Fine-tuning/evaluation policy and model promotion remain later M09 decisions and require explicit evidence rather than automatic weight mutation.
- No embeddings, model-specific core behavior, provider fallback, or provider token hard cap is introduced.

## Immediate next action

Implement `tasks/M09-001.md`: one deterministic read-only trainer-neutral snapshot of the complete Active canonical Controller experience dataset, preserving exact M08 fields and stable ordering without specialization policy.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
