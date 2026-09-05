# Orc Next Roadmap

The roadmap defines direction, not a frozen implementation plan. Only the current and next milestone should be decomposed deeply.

## M00 — Architecture and repository mapping — COMPLETE

Mapped the existing repository against the Controller/kernel target before changing architecture.

Result: `M00-REPOSITORY-MAP.md` identifies deterministic kernel surfaces, judgment/policy migration targets, reusable `OrcApp` / `ProjectOperations` seams, Lead/Planner migration boundaries, and the minimal read-only Controller integration seam.

## M01 — Native model runtime — COMPLETE

Integrate the local Controller inference boundary. Initial target: Qwen3 8B + llama.cpp/GGUF, while keeping model-specific details replaceable.

Exit criteria:
- a small model-independent native runtime interface exists;
- Qwen3/llama.cpp/GGUF-specific concerns stay behind the adapter;
- model location/configuration and failure reporting are explicit;
- Orc can perform a bounded local inference request and receive text/structured output;
- no lifecycle/database mutation is granted to the model;
- deterministic tests cover the runtime boundary without requiring the real model;
- a real local smoke test is documented separately from normal deterministic tests;
- no Python runtime dependency is introduced.

Result: M01-002 received source review **PASS**. The opt-in `Qwen3-8B-Q4_K_M.gguf` smoke passed in `15.26s` (`1 passed; 0 failed`) through `LocalInferenceRuntime` → `LlamaCppRuntime` → llama.cpp on CPU. Vulkan/GPU optimization remains a separate concern.

## M02 — Read-only Controller — COMPLETE

Give the Controller bounded project/task/validation/review/agent state. It recommends next actions but cannot mutate state.

## M03 — Typed Controller tools — COMPLETE

Expose a small high-level tool/action surface over canonical Orc APIs. Kernel validates every intent.

## M04 — Recovery intelligence — COMPLETE

Move retry, validation-failure response, unusual recovery and escalation judgment into Controller reasoning. Remove superseded rigid policy rather than layering intelligence on top of it.

Result: bounded recovery observation/legality, read-only Controller recovery choice, one-shot supervised authorization/execution, validation-repair exhaustion migration, and semantic revision non-convergence migration are complete. Deterministic validation/review/revision/economy facts remain kernel-owned; migrated post-failure recovery choices no longer automatically invoke economy escalation.

## M05 — Planning and Lead unification — COMPLETE

Move planning and Lead-like judgment into Controller. Preserve useful Plan/approval data while simplifying obsolete role/handoff machinery.

Result: supervised Controller workflow now owns Plan generation/review/revision and intake judgment while deterministic kernel code retains persistence, workflow routing, approval/application gates, validation, authorization, and lifecycle invariants.

## M06 — Persistent memory — COMPLETE

Add user, project, episodic and experience memory, consolidation judgment, provenance and retrieval.

Result: M06-001 established typed durable User/Project/Episodic/Experience records and canonical project/global persistence. M06-002 established reusable deterministic bounded read-only `ControllerMemoryContext`. M06-003 through M06-008 integrated bounded memory into all currently identified Controller read/judgment seams while preserving current-facts authority and capability-local request types. M06-009 established canonical supervised Create/Correct/Supersede/Remove mutation intents, deterministic legality, one-shot authorization, fresh-state validation, and execution through `MemoryService`. M06-010 added explicit-candidate capture judgment for Create. M06-011 added explicit-target maintenance judgment for Keep/Correct/Supersede/Remove.

M06 intentionally stops before automatic invocation. Capture and maintenance remain explicitly invoked and supervised. No background memory scan, transcript ingestion, autonomous consolidation, semantic/vector retrieval, embeddings, learned ranking, or model-specific memory behavior is introduced. Automatic safe continuation and invocation decisions belong to M07; semantic retrieval remains evidence-driven future work if deterministic bounded retrieval proves insufficient.

## M07 — Supervised autonomy — CURRENT

Allow routine safe continuation inside explicit operator permissions and budgets.

Orc already has the canonical pieces M07 must compose rather than replace: bounded Controller judgment seams, exact typed action/memory intents, one-shot authorization/execution with fresh legality, finite project-bound grants, and the restart-safe `WorkflowEngine` with finite plan/task revision and transition limits.

M07-001 through M07-005 are complete. They established bounded routine task continuation without a second orchestration engine: opaque finite task-action grants, one-step composition, exact expected-action enforcement, one-edge workflow routing, and repeated grant-aware continuation through the existing `continue_run_inner()` loop. Deterministic/routing/planning edges consume no routine-action budget; Dispatch/Review/Revision consume at most one each. Acceptance, user gates, external waits, transition limits, revision limits, and persisted workflow state remain authoritative.

After M07-005 there is no remaining repository-grounded routine-task continuation seam.

M07-006 is complete. It established `ControllerMemoryCaptureGrant` as a separate finite permission domain for durable capture rather than broadening `ControllerContinuationGrant`. The grant is opaque, project-bound, in-process, clone-shared, explicitly revocable, capped at 128 actions, and never persisted/reconstructed. It can mint only the existing M06-009 exact one-shot authorization for exact-current-project `Project` or `Episodic` Create proposals. User/Experience and Correct/Supersede/Remove remain outside this automatic-capable permission. Pre-mint rejection consumes zero; successful authorization mint consumes one; post-mint failure is not refunded. M06-009 remains the only canonical mutation execution path.

With permission established, the next seam is composition rather than automatic event wiring. M06-010 already judges one explicit candidate; M06-009 already proposes and executes; M07-006 already gates authorization. These should first be proven end-to-end through one explicit application operation before Orc begins deriving candidates automatically from workflow outcomes.

M07-007 is the next task: compose exactly one caller-supplied `ControllerMemoryCaptureRequest` through existing M06-010 judgment → M06-009 proposal → M07-006 grant inspection → M06-009 canonical execution. One call performs at most one inference, one proposal, one authorization mint, and one mutation attempt. `Ignore` and all pre-mint failure/rejection consume zero grant units; a successful authorization consumes one; post-mint failure is not refunded. No retry, fallback, direct memory mutation, second loop, or automatic candidate derivation is allowed.

M07-007 still does not attach capture to workflow/task/Plan/review/validation/recovery/transcript/lifecycle events. Automatic candidate derivation/invocation is a separate later M07 decision after this one-step chain is proven. Automatic User/Experience writes and automatic Correct/Supersede/Remove maintenance remain separate higher-risk capabilities. No background scanning, semantic/vector retrieval, embeddings, model-specific behavior, or provider token hard cap is introduced.

## M08 — Experience dataset

Turn verified Controller decisions, corrections and outcomes into a curated evaluation/training dataset.

## M09 — Controller specialization

Fine-tune/evaluate the local Controller model. A new model becomes default only when evaluation shows it improves Controller behavior without unacceptable regressions.

## M10 — Interface integration

Expose the mature Controller consistently through CLI, TUI and GUI using shared core APIs.
