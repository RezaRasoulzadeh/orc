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

## M06 — Persistent memory — CURRENT

Add user, project, episodic and experience memory, consolidation, provenance and retrieval.

M06-001 established typed durable memory and canonical project/global persistence. M06-002 established reusable deterministic bounded read-only `ControllerMemoryContext`. M06-003 through M06-008 integrated it into Plan generation, recovery recommendation, normal task recommendation, workflow intake, Plan review, and Plan revision through capability-local inputs that preserve current-facts authority and deterministic kernel boundaries.

M06-009 established the supervised write boundary: typed Create/Correct/Supersede/Remove intents, deterministic legality, trusted one-shot authorization, fresh-state revalidation, and execution through existing M06-001 APIs. M06-010 then established explicit-candidate capture judgment: one bounded candidate can be ignored or proposed as a canonical Create intent without authorization or execution.

The remaining identified M06 seam is M06-011: supervised maintenance/consolidation judgment over one explicitly selected active memory target. It should reuse `ControllerMemoryContext` plus M06-009 Correct/Supersede/Remove intents, keep all mutation supervised, and avoid automatic scanning or background cleanup. If M06-011 closes without exposing another repository-grounded gap, M06 can complete there.

Automatic invocation of capture/maintenance belongs with later supervised-autonomy work rather than being smuggled into persistent-memory infrastructure. Semantic/vector retrieval likewise remains later and should only be introduced with evidence that deterministic bounded retrieval is insufficient.

## M07 — Supervised autonomy

Allow routine safe continuation inside explicit operator permissions and budgets.

## M08 — Experience dataset

Turn verified Controller decisions, corrections and outcomes into a curated evaluation/training dataset.

## M09 — Controller specialization

Fine-tune/evaluate the local Controller model. A new model becomes default only when evaluation shows it improves Controller behavior without unacceptable regressions.

## M10 — Interface integration

Expose the mature Controller consistently through CLI, TUI and GUI using shared core APIs.
