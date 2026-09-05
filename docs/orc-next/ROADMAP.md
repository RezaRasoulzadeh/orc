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

Orc already has the canonical pieces M07 must compose rather than replace: bounded `OrcApp::propose_controller_action` recommendation, exact typed M03 action intents and one-shot authorization/execution with fresh legality, the M07-001 finite project-bound continuation grant, the restart-safe `WorkflowEngine` with finite plan/task revision and transition limits, and the M06-009/M06-010 supervised durable-memory boundaries.

M07-001 is complete. It established opaque project-bound grants over only Dispatch/SemanticReview/Revise, with a finite 1–128 action budget, shared anti-reset state, Active/Exhausted/Revoked lifecycle, current legality inspection, exact M03 authorization reuse, and deterministic exclusion of Accept.

M07-002 is complete. It established `OrcApp::continue_controller_action_once()`, composing one existing Controller recommendation/proposal, exact task validation, M07-001 grant inspection, and the exact existing M03 execution boundary. One call performs at most one inference, one successful grant consumption, and one routine action, with no retry, task enumeration, automatic acceptance, or second orchestration loop.

M07-003 is complete. The one-step seam now accepts a trusted existing expected action. Only Dispatch/SemanticReview/Revise are valid; expected Accept and mismatched Controller recommendations stop before grant inspection, preserving zero budget consumption and preventing an action-specific execution-context mismatch from burning permission the workflow stage never granted.

M07-004 is complete. `WorkflowEngine::continue_one_with_controller_grant()` integrates one persisted Controller task edge with that supervised chain. `Dispatch`, `Review`, and `Revision` map deterministically to the existing action kinds; exact `current_task_id` remains workflow-owned; the production adapter maps successful canonical execution evidence back to existing `ProviderOutcome` / `ReviewOutcome`; and any supervised rejection/failure stops without fallback provider execution, retry, or second inference. Acceptance and non-task stages remain outside this single-edge entry point.

M07-005 is complete. `WorkflowEngine::continue_run_with_controller_grant()` reuses the existing bounded `continue_run_inner()` loop rather than creating another autonomy engine. Deterministic/routing/planning edges consume zero continuation units; each persisted Dispatch/Review/Revision edge reuses M07-004 and can consume exactly one grant unit. Exhausted/revoked/wrong-project/unusable grants stop before another routine Controller inference or action with no fallback. Workflow transition and revision limits remain independent. Acceptance, WaitingUser, WaitingExternal and other non-running boundaries stop the grant-aware path; even ordinary automatic-acceptance policy is not crossed by this supervised API, while existing non-grant behavior is preserved.

After M07-005 there is no remaining repository-grounded routine-task continuation seam. The next M07 gap comes from M06's deliberate deferral of automatic memory invocation: M06-010 can judge one exact capture candidate, and M06-009 can authorize/execute one exact memory mutation, but there is no finite operator permission that can safely bridge repeated future automatic capture decisions to durable writes.

M07-006 is the next task: establish a capability-specific, finite, project-bound Controller memory-capture grant. It must not broaden `ControllerContinuationGrant` into a universal permission token. The capture grant may mint only the existing M06-009 one-shot authorization for exact eligible `Create` proposals targeting project-bound `Project` or `Episodic` memory in the current project. Global `User`/`Experience` writes and Correct/Supersede/Remove maintenance remain outside this permission. Rejected inspection consumes zero budget; successful authorization mint consumes one; post-mint failure is not refunded. The grant remains in-process and non-persistent.

M07-006 still does not derive candidates from workflow/task/review events, automatically invoke capture judgment, automatically mutate memory, automate maintenance, add background scanning, or change memory retrieval. Those remain separate decisions after the bounded capture-permission seam is proven.

## M08 — Experience dataset

Turn verified Controller decisions, corrections and outcomes into a curated evaluation/training dataset.

## M09 — Controller specialization

Fine-tune/evaluate the local Controller model. A new model becomes default only when evaluation shows it improves Controller behavior without unacceptable regressions.

## M10 — Interface integration

Expose the mature Controller consistently through CLI, TUI and GUI using shared core APIs.
