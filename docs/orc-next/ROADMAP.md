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

M08-002 is complete. It established capability-local curation for normal task recommendation with fixed capability `controller.task_recommendation`, exact validated `ControllerRecommendationInput`, canonical structured recommendation output, explicit correction semantics, and persistence only through M08-001.

M08-003 is complete. It established capability-local curation for recovery recommendation with fixed capability `controller.recovery_recommendation`, exact validated `RecoveryInferenceInput`, exact accepted `RecoveryRecommendation`, explicit correction semantics, and persistence only through M08-001. `RecoveryRecommendationValidation` remains deterministic runtime legality/actionability evidence rather than a dataset reasoning target.

M08-004 is complete. It established capability-local curation for Controller planning with fixed capability `controller.plan_generation`, exact validated `ControllerPlanningInput`, and exact complete accepted `ControllerPlanResult`, preserving its canonical `PlanResponse`, rationale, and optional uncertainty. Planning inference behavior and downstream Plan/workflow semantics remain unchanged.

M08-005 is complete. It established capability-local curation for Controller workflow intake with fixed capability `controller.workflow_intake`, exact validated `ControllerIntakeInput`, and exact complete accepted `ControllerIntakeResult`, preserving DirectTasks proposals and decision semantics without deriving labels from routing/application/outcomes.

M08-006 is complete. It established capability-local curation for Controller Plan review with fixed capability `controller.plan_review`, exact validated `ControllerPlanReviewInput`, and exact complete accepted `ControllerPlanReviewResult`. Persisted PlanReview rows, Plan status transitions, later revisions/approvals, task application, workflow advancement, and downstream outcomes remain outside dataset verification.

M08-007 now targets Controller Plan revision generation. The production inference boundary consumes `ControllerPlanRevisionInput` and returns a canonical generated `PlanResponse`; only after inference does trusted application code construct `ControllerPlanRevisionResult` by attaching parent Plan ID/version and review ID.

Therefore M08-007 must use the exact generated/accepted `PlanResponse` as the dataset output authority and must exclude trusted lineage from the reasoning target. The exact typed revision input, including current Plan, persisted revision feedback, planning context, and bounded memory, remains the dataset input authority.

M08-007 must not infer labels from revised-Plan persistence, lineage consistency, subsequent Plan review, task application, workflow advancement, validation/review, operator acceptance, or eventual task/workflow outcomes. It adds no automatic revision hook, generic capture framework, dataset export, balancing/splitting, embeddings, fine-tuning, Python runtime dependency, model-specific behavior, provider fallback, or provider token hard cap.

Later M08 tasks should continue capability-local curation only where existing typed contracts make the projection unambiguous, then add deterministic dataset inspection/export/evaluation preparation as repository evidence warrants.

## M09 — Controller specialization

Fine-tune/evaluate the local Controller model. A new model becomes default only when evaluation demonstrates improvement without unacceptable regressions.

## M10 — Interface integration

Expose the mature Controller consistently through CLI, TUI and GUI using shared core APIs.
