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

The next repository-grounded gap is maintenance target selection. `MemoryService::list(...)` can deterministically enumerate current-project records, but choosing which active record warrants maintenance is judgment and must not be hardcoded into the kernel.

M07-010 adds bounded read-only Controller maintenance target selection. Trusted code deterministically supplies only active exact-current-project Project/Episodic candidates plus explicit current facts; the Controller returns no target or one exact candidate. Deterministic code owns filtering, ordering, bounds, and output validation. M06-011 continues to own Keep/Correct/Supersede/Remove judgment for the selected target.

M07-010 does not yet invoke maintenance automatically, derive current facts from workflow events, inspect grants, mutate memory, scan in background, batch records, or touch User/Experience/global memory. Because it introduces a new inference schema, focused real-Qwen evaluation is required.

## M08 — Experience dataset

Turn verified Controller decisions, corrections and outcomes into a curated evaluation/training dataset.

## M09 — Controller specialization

Fine-tune/evaluate the local Controller model. A new model becomes default only when evaluation demonstrates improvement without unacceptable regressions.

## M10 — Interface integration

Expose the mature Controller consistently through CLI, TUI and GUI using shared core APIs.
