# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M08 — Experience dataset

**Current task:** M08-001 — Establish typed verified Controller experience examples

**Last completed:** M07-011 — Compose one selected Controller memory maintenance step

**Blocked by:** Nothing

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M06 is complete: typed durable memory, bounded deterministic retrieval, capability-local memory context, supervised mutation, explicit-candidate capture judgment, and explicit-target maintenance judgment.
- M07 is complete: finite grant-aware routine task continuation, supervised Project/Episodic capture Create, explicit-target maintenance, bounded Controller maintenance-target selection, and one selected-target maintenance composition.
- M07-011 preserves the exact caller-supplied current facts across selection and maintenance, carries only canonical `MemoryId` across that boundary, freshly re-resolves target state through M06-011/M07-009, and adds no retry/fallback/second loop.
- Automatic workflow-derived memory facts/candidates remain intentionally excluded. Deterministic application code must not invent what should be remembered or what evidence warrants maintenance merely to enable automation.
- M08 now begins the curated Controller experience dataset required by D-006.
- Runtime `MemoryKind::Experience` remains distinct from training/evaluation dataset examples. Runtime memory is for Controller context; the M08 dataset is curated reasoning evidence.
- M08-001 establishes the first typed/versioned persistent verified-example record in the existing global registry database with explicit trusted creation only.
- M08-001 must not automatically classify executed/accepted/reviewed/validated Controller behavior as verified training data. Verification evidence remains explicit until later curation tasks define narrower capture seams.
- No model training, export, embeddings, Python runtime dependency, or provider token hard cap is introduced by M08-001.

## Immediate next action

Implement `tasks/M08-001.md`: establish the canonical typed persistent verified Controller experience-example record and explicit trusted create/get/list/retire APIs, distinct from runtime Experience memory and without automatic harvesting.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
