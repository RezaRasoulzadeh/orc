# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M08 — Experience dataset

**Current task:** M08-003 — Add explicit typed curation for Controller recovery recommendations

**Last completed:** M08-002 — Add explicit typed curation for normal Controller recommendations

**Blocked by:** Nothing

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M06 and M07 are complete.
- M08 is the curated Controller experience dataset required by D-006 and remains distinct from runtime `MemoryKind::Experience`.
- M08-001 established the canonical typed/versioned global-registry experience-example substrate with explicit trusted creation, bounded validation, provenance, correction/outcome/quality metadata, deterministic query, and active/retired lifecycle.
- M08-002 is complete at implementation `adfac96dd5c8f77ef7d627858beb8a9aa58ded3b`.
- M08-002 adds explicit typed curation for normal task recommendation only, with fixed capability `controller.task_recommendation`, exact `ControllerRecommendationInput` projection, canonical structured recommendation output, explicit correction semantics, and persistence only through M08-001.
- M08-002 exposed reusable `ControllerRecommendation::validate()` without changing the recommendation prompt/schema/parser/bounds or runtime behavior.
- Verification, accepted output, quality, correction, and provenance remain explicit trusted inputs. Execution/workflow/review/validation success does not automatically label dataset examples.
- The next smallest proven seam is recovery curation. Recovery already has bounded typed `RecoveryInferenceInput` and typed `RecoveryRecommendation`, so M08-003 can remain capability-local rather than introducing a generic experience-capture framework.
- M08-003 must preserve the exact recovery inference input and exact accepted canonical recovery recommendation. `RecoveryRecommendationValidation` remains runtime legality/actionability evidence, not the dataset target.
- No automatic harvesting, workflow/recovery hook, label inference, runtime-memory change, export, splitting/balancing, embeddings, fine-tuning, Python runtime dependency, model-specific behavior, or provider token hard cap is introduced.

## Immediate next action

Implement `tasks/M08-003.md`: explicit typed curation of one already-produced Controller recovery recommendation interaction into the canonical M08-001 dataset format.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
