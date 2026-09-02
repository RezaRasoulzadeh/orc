# Orc Next Roadmap

The roadmap defines direction, not a frozen implementation plan. Only the current and next milestone should be decomposed deeply.

## M00 — Architecture and repository mapping — CURRENT

Map the existing repository against the Controller/kernel target before changing architecture.

Exit criteria:
- deterministic kernel surfaces identified;
- existing judgment/policy surfaces identified;
- reusable `OrcApp` / `ProjectOperations` seams identified;
- Lead/Planner data versus role-specific machinery mapped;
- lifecycle invariants separated from policy;
- economy constraints separated from judgment;
- validation-repair/recovery policy mapped;
- minimal read-only Controller integration seam proposed;
- no broad rewrite introduced.

## M01 — Native model runtime

Integrate the local Controller inference boundary. Initial target: Qwen3 8B + llama.cpp/GGUF, while keeping model-specific details replaceable.

## M02 — Read-only Controller

Give the Controller bounded project/task/validation/review/agent state. It recommends next actions but cannot mutate state.

## M03 — Typed Controller tools

Expose a small high-level tool/action surface over canonical Orc APIs. Kernel validates every intent.

## M04 — Recovery intelligence

Move retry, validation-failure response, unusual recovery and escalation judgment into Controller reasoning. Remove superseded rigid policy rather than layering intelligence on top of it.

## M05 — Planning and Lead unification

Move planning and Lead-like judgment into Controller. Preserve useful Plan/approval data while simplifying obsolete role/handoff machinery.

## M06 — Persistent memory

Add user, project, episodic and experience memory, consolidation, provenance and retrieval.

## M07 — Supervised autonomy

Allow routine safe continuation inside explicit operator permissions and budgets.

## M08 — Experience dataset

Turn verified Controller decisions, corrections and outcomes into a curated evaluation/training dataset.

## M09 — Controller specialization

Fine-tune/evaluate the local Controller model. A new model becomes default only when evaluation shows it improves Controller behavior without unacceptable regressions.

## M10 — Interface integration

Expose the mature Controller consistently through CLI, TUI and GUI using shared core APIs.
