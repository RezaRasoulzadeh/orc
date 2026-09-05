# Orc Next Roadmap

The roadmap defines direction, not a frozen implementation plan. Only the current and next milestone should be decomposed deeply.

## M00 — Architecture and repository mapping — COMPLETE

Mapped the existing repository against the Controller/kernel target before changing architecture.

Result: `M00-REPOSITORY-MAP.md` identifies deterministic kernel surfaces, judgment/policy migration targets, reusable `OrcApp` / `ProjectOperations` seams, Lead/Planner migration boundaries, and the minimal read-only Controller integration seam.

## M01 — Native model runtime — COMPLETE

Integrate the local Controller inference boundary. Initial target: Qwen3 8B + llama.cpp/GGUF, while keeping model-specific details replaceable.

Result: M01-002 received source review **PASS**. The opt-in `Qwen3-8B-Q4_K_M.gguf` smoke passed through `LocalInferenceRuntime` → `LlamaCppRuntime` → llama.cpp on CPU. Vulkan/GPU optimization remains separate.

## M02 — Read-only Controller — COMPLETE

Give the Controller bounded project/task/validation/review/agent state. It recommends next actions but cannot mutate state.

## M03 — Typed Controller tools — COMPLETE

Expose a small high-level tool/action surface over canonical Orc APIs. Kernel validates every intent.

## M04 — Recovery intelligence — COMPLETE

Move retry, validation-failure response, unusual recovery and escalation judgment into Controller reasoning while deterministic validation/review/revision/economy facts remain kernel-owned.

## M05 — Planning and Lead unification — COMPLETE

Move planning and Lead-like judgment into Controller while preserving deterministic persistence, workflow routing, approval/application gates, validation, authorization, and lifecycle invariants.

## M06 — Persistent memory — COMPLETE

M06 established typed durable User/Project/Episodic/Experience records, deterministic bounded `ControllerMemoryContext`, capability-local memory integration, supervised Create/Correct/Supersede/Remove mutation, explicit-candidate capture judgment, and explicit-target maintenance judgment.

M06 intentionally stops before automatic invocation. No background memory scan, transcript ingestion, autonomous consolidation, semantic/vector retrieval, embeddings, learned ranking, or model-specific memory behavior is introduced.

## M07 — Supervised autonomy — CURRENT

Allow routine safe continuation inside explicit operator permissions and budgets.

M07-001 through M07-005 are complete. They established bounded routine task continuation without a second orchestration engine: finite task-action grants, one-step composition, expected-action enforcement, one-edge workflow routing, and repeated grant-aware continuation through the existing workflow loop. Acceptance, user gates, external waits, transition limits, revision limits, and persisted workflow state remain authoritative.

M07-006 is complete. It established `ControllerMemoryCaptureGrant` as a separate finite permission domain for project-bound Project/Episodic Create. The grant is opaque, in-process, clone-shared, explicitly revocable, capped at 128 actions, and never persisted/reconstructed. User/Experience and maintenance operations remain outside capture permission.

M07-007 is complete. `OrcApp::capture_controller_memory_once(...)` composes one explicit capture request through M06-010 judgment → M06-009 proposal → M07-006 grant inspection → M06-009 canonical execution. One call performs at most one inference, one proposal, one authorization mint, and one execution attempt. Ignore and all pre-mint failure/rejection consume zero; successful authorization consumes one; post-mint failure is not refunded. The public result is state-safe and cannot represent impossible success/rejection combinations.

Automatic capture candidate derivation is still intentionally unresolved. `ControllerMemoryCaptureRequest` already contains a full `MemoryDraft`; synthesizing durable subject/content/kind/scope deterministically from workflow state would hardcode the judgment of what should be remembered. M07 must not introduce that policy merely to create an automatic hook.

The next repository-grounded gap is the other existing M06 durable-memory judgment domain: M06-011 maintenance. It can judge one explicit active target and produce Keep or exact-target Correct/Supersede/Remove, but there is no bounded operator permission analogous to M07-006 for allowing those maintenance proposals to obtain M06-009 authorization.

M07-008 is the next task: establish a separate finite project-bound Controller memory-maintenance grant. It may mint only the existing M06-009 exact one-shot authorization for already validated Correct/Supersede/Remove proposals targeting exact-current-project Project/Episodic memory. Create and global User/Experience maintenance remain excluded. Rejected inspection consumes zero; successful authorization consumes one; post-mint failure is not refunded. The grant remains opaque, in-process, clone-shared, revocable, and non-persistent.

M07-008 adds permission only. Automatic target selection, maintenance invocation, one-step maintenance composition, workflow/lifecycle hooks, background cleanup, batch consolidation, semantic/vector retrieval, embeddings, model-specific behavior, and provider token hard caps remain out of scope.

## M08 — Experience dataset

Turn verified Controller decisions, corrections and outcomes into a curated evaluation/training dataset.

## M09 — Controller specialization

Fine-tune/evaluate the local Controller model. A new model becomes default only when evaluation shows it improves Controller behavior without unacceptable regressions.

## M10 — Interface integration

Expose the mature Controller consistently through CLI, TUI and GUI using shared core APIs.
