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

M07-010 added bounded read-only Controller maintenance-target selection over deterministic canonical active current-project Project/Episodic candidates. Filtering, ordering, bounds, omission metadata, and exact-candidate validation remain deterministic. Focused real-Qwen evaluation passed strict 7/7 and semantic 7/7.

M07-011 completed the selected-target maintenance chain: one selector call may choose one exact canonical `MemoryId`, then one existing maintenance call freshly re-resolves that target and reuses the exact unchanged caller-supplied current facts. The composition performs at most two Controller inference calls, one proposal, one authorization mint, and one execution attempt, with no retry, alternate target, omitted-candidate fallback, direct mutation, or second orchestration loop.

M07 intentionally stops short of automatic workflow-derived memory facts/candidates. `ControllerMemoryCaptureRequest` already requires a full `MemoryDraft`, and maintenance selection requires explicit current facts. Deterministic workflow code must not invent what should be remembered or what evidence warrants maintenance merely to create an automatic hook. That remains a judgment boundary rather than an unfinished kernel invariant.

## M08 — Experience dataset — CURRENT

Turn verified Controller decisions, corrections, and outcomes into a curated evaluation/training dataset as required by D-006.

M08 keeps dataset examples distinct from runtime `MemoryKind::Experience`. Runtime Experience memory is retrieved as Controller context; curated dataset records are evidence for evaluation/training and must not masquerade as memory or current project truth.

M08-001 establishes the first canonical typed/versioned persistent verified Controller experience-example record in the existing global registry database. Creation is explicit trusted application/kernel work only. The record carries bounded input, accepted/expected output, verification basis, optional correction/outcome metadata, and source/project provenance. It supports deterministic create/get/list/retire operations while preserving history/provenance.

M08-001 does not automatically harvest Controller traffic, infer labels from workflow success, validation/review/acceptance, export a training corpus, balance/split data, train a model, add embeddings, or introduce model-specific behavior. Later M08 tasks should build only on repository evidence after this canonical dataset substrate is proven.

## M09 — Controller specialization

Fine-tune/evaluate the local Controller model. A new model becomes default only when evaluation demonstrates improvement without unacceptable regressions.

## M10 — Interface integration

Expose the mature Controller consistently through CLI, TUI and GUI using shared core APIs.
