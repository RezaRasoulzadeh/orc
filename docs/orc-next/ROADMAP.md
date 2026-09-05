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

## M07 — Supervised autonomy — CURRENT

M07-001 through M07-005 established finite routine task-action grants and grant-aware continuation through the existing workflow loop without bypassing Acceptance, user/external gates, transition limits, revision limits, or persisted workflow state.

M07-006/M07-007 established a distinct finite capture permission and one-step explicit capture composition for exact-current-project Project/Episodic Create. Automatic capture candidate derivation remains intentionally unresolved because `ControllerMemoryCaptureRequest` contains a full `MemoryDraft`; deterministic workflow code must not invent durable memory content.

M07-008/M07-009 established a distinct finite maintenance permission and one-step explicit maintenance composition for exact-current-project Project/Episodic Correct/Supersede/Remove. `OrcApp::maintain_controller_memory_once(...)` composes one explicit request through M06-011 judgment → M06-009 proposal → M07-008 grant → M06-009 execution, with at most one inference/proposal/authorization/execution and state-safe results. Keep/pre-mint failures consume zero; successful mint consumes one; post-mint failure is not refunded.

M07-010 is complete. `OrcApp::select_controller_memory_target(...)` performs bounded read-only maintenance target selection from deterministic canonical active exact-current-project Project/Episodic candidates. Filtering, ordering, bounds, omission metadata, and exact-candidate output validation remain deterministic; the Controller returns only no target or one supplied target. User/Experience/global/historical/cross-project records remain excluded. Focused real-Qwen evaluation passed strict 7/7 and semantic 7/7.

The next smallest safe step is composition, not event automation. M07-011 composes exactly one M07-010 selection with at most one existing M07-009 maintenance call. The caller still supplies the authoritative bounded `current_facts` and an explicit finite maintenance grant. The exact same facts must flow unchanged into both target selection and M06-011 maintenance judgment.

A selected target remains advisory until M06-011/M07-009 freshly re-resolves its canonical identity and current active state. M07-011 must not cache the M07-010 candidate record as mutation authority. One composed call may perform at most two Controller inference calls total, one mutation proposal, one authorization mint, and one execution attempt. No retry, alternate target, omitted-candidate fallback, batch loop, or direct mutation is permitted.

M07-011 still does not derive current facts from workflow/task/Plan/review/validation/recovery events and does not add background maintenance or lifecycle hooks. Automatic fact derivation/event integration is a separate later M07 decision. Automatic capture candidate derivation remains unresolved for the same judgment-boundary reason. User/Experience/global maintenance, semantic/vector retrieval, embeddings, learned ranking, model-specific behavior, and provider token hard caps remain out of scope.

## M08 — Experience dataset

Turn verified Controller decisions, corrections and outcomes into a curated evaluation/training dataset.

## M09 — Controller specialization

Fine-tune/evaluate the local Controller model. A new model becomes default only when evaluation demonstrates improvement without unacceptable regressions.

## M10 — Interface integration

Expose the mature Controller consistently through CLI, TUI and GUI using shared core APIs.
