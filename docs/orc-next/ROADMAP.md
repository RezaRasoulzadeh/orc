# Orc Next Roadmap

The roadmap defines direction, not a frozen implementation plan. Only the current and next milestone should be decomposed deeply.

## M00 — Architecture and repository mapping — COMPLETE

Mapped the repository against the Controller/kernel target and established canonical application/observation seams.

## M01 — Native model runtime — COMPLETE

Model-independent local runtime with llama.cpp/GGUF Qwen3 8B integration.

## M02 — Read-only Controller — COMPLETE

Bounded project/task state and structured read-only Controller recommendations.

## M03 — Typed Controller tools — COMPLETE

Typed intents, deterministic legality, explicit authorization, canonical execution.

## M04 — Recovery intelligence — COMPLETE

Controller recovery judgment over deterministic failure/recovery facts.

## M05 — Planning and Lead unification — COMPLETE

Controller planning/intake/Plan review/revision with deterministic persistence and workflow gates.

## M06 — Persistent memory — COMPLETE

Typed User/Project/Episodic/Experience persistence, deterministic bounded `ControllerMemoryContext`, capability-local integration, supervised mutation, explicit-candidate capture judgment, and explicit-target maintenance judgment. Memory remains separate from model weights. No background scan, transcript ingestion, vector retrieval, embeddings, or autonomous consolidation.

## M07 — Supervised autonomy — COMPLETE

M07 established finite routine task-action grants and grant-aware continuation through the existing workflow loop without bypassing Acceptance, user/external gates, transition limits, revision limits, or persisted workflow state.

It also established separate finite memory capability permissions: Project/Episodic Create through capture grants, and Project/Episodic Correct/Supersede/Remove through maintenance grants. Explicit one-step capture and maintenance composition reuse M06-009 as the sole durable mutation boundary.

M07-010 added bounded read-only Controller maintenance-target selection over deterministic canonical active current-project Project/Episodic candidates. M07-011 completed one selected-target maintenance chain while preserving caller-supplied current facts and fresh target resolution. M07 intentionally stops short of automatic workflow-derived memory facts/candidates because deterministic code must not invent what should be remembered or what evidence warrants maintenance.

## M08 — Experience dataset — CURRENT

Turn verified Controller decisions, corrections, and outcomes into a curated evaluation/training dataset as required by D-006.

M08 keeps dataset examples distinct from runtime `MemoryKind::Experience`. Runtime Experience memory is retrieved as Controller context; curated dataset records are evidence for evaluation/training and must not masquerade as memory or current project truth.

M08-001 is complete. It established canonical typed/versioned `ControllerExperienceExampleDraft` and `ControllerExperienceExample` records in the existing global registry, with bounded canonical input/accepted-output payloads, verification basis, correction/outcome/quality metadata, provenance, deterministic bounded query, and active/retired lifecycle. Creation remains explicit trusted application work only; no automatic harvesting or verification policy exists.

M08-002 now adds the first capability-local curation adapter for a real Controller reasoning surface: normal task recommendation. It must project the existing validated `ControllerRecommendationInput` and existing canonical structured recommendation result into M08-001 without parallel schemas or caller-controlled capability labels. Accepted/corrected output, verification, quality, and provenance remain explicit trusted inputs.

M08-002 does not infer labels from workflow execution, validation, review, acceptance, or task success. It adds no inference, runtime hook, automatic harvesting, dataset export, balancing/splitting, embeddings, fine-tuning, Python runtime dependency, or model-specific behavior.

Later M08 tasks should expand curation across additional Controller capabilities only after this typed projection seam is proven, then add deterministic dataset inspection/export/evaluation preparation as repository evidence warrants.

## M09 — Controller specialization

Fine-tune/evaluate the local Controller model. A new model becomes default only when evaluation demonstrates improvement without unacceptable regressions.

## M10 — Interface integration

Expose the mature Controller consistently through CLI, TUI and GUI using shared core APIs.
