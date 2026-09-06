# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M08 — Experience dataset

**Current task:** M08-002 — Add explicit typed curation for normal Controller recommendations

**Last completed:** M08-001 — Establish typed verified Controller experience examples

**Blocked by:** Nothing

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M06 and M07 are complete.
- Automatic workflow-derived memory facts/candidates remain intentionally excluded; deterministic code must not invent memory evidence merely to create automation.
- M08 is the curated Controller experience dataset required by D-006.
- Runtime `MemoryKind::Experience` remains distinct from M08 dataset examples. Runtime memory is Controller context; M08 records are curated reasoning evidence for evaluation/training.
- M08-001 is complete at implementation `c4af7f72c739aabb59ea618925a02eb6244f6574`.
- M08-001 provides typed/versioned global-registry examples, explicit verification/correction/outcome/quality/provenance metadata, hard bounds, deterministic create/get/list/retire APIs, and no automatic inference or harvesting.
- M08-002 adds the smallest capability-local curation adapter for normal Controller task recommendation only.
- The adapter must use the existing typed `ControllerRecommendationInput` and existing canonical recommendation structured output rather than parallel schemas or caller-built arbitrary JSON.
- Verification, accepted output, quality, correction, and provenance remain explicit trusted inputs. Workflow success, validation/review, acceptance, or execution must not automatically label an example.
- M08-002 is explicit curation only: no inference, workflow hook, automatic harvesting, dataset export, splitting/balancing, embeddings, training, model-specific behavior, Python runtime dependency, or provider token hard cap.

## Immediate next action

Implement `tasks/M08-002.md`: explicit typed curation of one already-produced normal Controller recommendation interaction into the canonical M08-001 dataset format.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
