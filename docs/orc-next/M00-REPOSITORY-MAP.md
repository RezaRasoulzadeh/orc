# M00 — Current Repository Map

This map classifies the current Orc implementation for the local-Controller migration. It is intentionally evolutionary: preserve deterministic execution machinery, reuse existing application/read seams, and move engineering judgment rather than rewriting the system.

## Classification

### Deterministic kernel — preserve

| Surface | Current location | Role in Orc Next |
| --- | --- | --- |
| Durable state and transactions | `src/storage/` | Canonical facts, atomic mutations, run/review/validation/plan evidence, lifecycle events. Remains authoritative. |
| Task contracts and lifecycle state | `src/task.rs`, `src/state.rs`, `src/queue.rs` | Task identity, contracts, dependencies, queue/lifecycle facts and legal state. |
| Git/worktrees | `src/git.rs` | Deterministic repository mutation/isolation boundary. |
| Validation execution and evidence | `src/validation.rs` | Runs configured commands and classifies/persists factual outcomes. The Controller must never convert a failing result into success. |
| Agent registry and authorization facts | `src/registry.rs`, persistence in `src/storage/` | Agent definitions, capabilities, permissions, availability and quota facts. |
| Scheduling eligibility | `src/scheduler.rs` | Hard eligibility/permission/capability constraints stay deterministic. Ranking policy may later become Controller input. |
| Review evidence and revision contracts | `src/review.rs`, `src/automated.rs`, storage | Persist semantic review results, blockers, criteria, lineage and revision contracts. |
| Atomic execution completion | `src/agent.rs`, `src/automated.rs`, storage transaction APIs | Preserve exact run/evidence/worktree publication semantics. |
| Acceptance/cancellation | `src/app.rs` plus storage/lifecycle APIs | Explicit operator gates and legal mutations remain kernel-owned. |

### Existing application/read seams — reuse

`src/app.rs` already exposes `OrcApp`, which owns the database, repository path and event hub and composes workflow, Lead, planning and task operations. It is the existing mutation/application boundary and should be adapted rather than bypassed.

`src/operations.rs` already defines `ProjectOperations` explicitly as a provider-independent read boundary. Its current summaries include lifecycle/phase/next-step, executions, validation, semantic Review, blockers, execution conditions, economy resolution, escalation history, token usage and activity. This is the strongest existing source for Controller observation.

`src/read_model.rs` composes stable interface workspaces from those operations. TUI and desktop should continue consuming shared read/application APIs rather than receiving Controller-specific business logic.

### Judgment/policy — migrate toward Controller

| Current concern | Current surface | Migration direction |
| --- | --- | --- |
| Abnormal recovery choice | lifecycle/requeue/unblock/workflow branches across `app`, `workflow`, agent execution | Kernel exposes legal recovery operations; Controller chooses among them from facts/evidence. |
| Validation repair/retry choice | repair orchestration in automated execution/revision path | Validation remains deterministic; Controller decides whether another repair, worker retry, stronger worker, return-to-revision, infrastructure block or operator decision is justified. |
| Economy escalation | `agent.rs`, registry economy types, provider resolution/escalation persistence | Quota, permission and configured limits remain facts/constraints. Whether escalation is useful is Controller judgment. |
| Planning | planning protocol + `OrcApp::automated_plan*` / pending-plan flow | Becomes a Controller capability while persisted Plan/task/dependency data is retained where useful. |
| Lead reasoning | `src/lead.rs`, Lead provider invocation and Lead decisions | Lead as a separate intelligent role/handoff is a migration candidate. Generic operator decisions/proposals can replace role-specific judgment machinery. |
| Workflow continuation policy | `src/workflow.rs` | Preserve durable workflow evidence only where useful; rigid judgment-heavy continuation should not become a second Controller. |
| Agent/model preference among eligible choices | scheduler/economy/provider resolution | Hard eligibility stays deterministic; Controller may recommend selection/escalation using current quota/cost/outcome evidence. |

## Lead and Planner split

### Durable data worth preserving

- persisted plans and plan history;
- approved task proposals and resulting task/dependency relationships;
- operator approvals/rejections and provenance;
- provider/run evidence associated with planning;
- decisions that remain useful as generic operator-decision history.

### Machinery likely to become obsolete

- Lead as a separately configured reasoning persona/provider;
- Lead-specific handoff requirements used only to decide what happens next;
- Planner as a separately orchestrated intelligent role when the Controller can plan directly;
- rigid Lead → Planner → Lead continuation policy where no deterministic invariant requires the distinction.

Removal must be migration-driven. M00 does not delete any of it.

## Lifecycle: invariant versus policy

Keep deterministic:

```text
Ready -> dispatch -> Active
Active -> completed implementation + valid evidence -> Review
Review PASS -> AcceptanceReady -> explicit accept -> Done
Review REVISE -> RevisionRequired -> explicit revise -> Active
```

Also keep deterministic legality around cancellation, dependency satisfaction, worktree ownership, evidence freshness, review/revision lineage and acceptance.

Move toward Controller judgment:

- what to do after unusual execution/validation failure;
- whether a blocked task should retry, return to a previous actionable state, change worker/model, wait, or ask the operator;
- when repeated attempts are no longer informative;
- which legal recovery operation best preserves valid lineage.

The kernel should report legal operations, not invent semantic recovery policy.

## Validation: fact versus judgment

Deterministic facts:

- commands selected;
- exit status/stdout/stderr/diagnostics;
- failure classification;
- worktree fingerprint and freshness;
- passing/failing/infrastructure state;
- persisted validation evidence.

Controller judgment:

- whether a repair attempt is worthwhile;
- what diagnostic subset/context the worker needs;
- whether repeated failure indicates missing context, implementation error or infrastructure;
- whether to retry the same worker, choose another eligible worker, return to revision-required, or stop and ask the operator.

## Economy: constraint versus judgment

Deterministic constraints/facts:

- agent capability/permission/availability;
- observed quota and reset time;
- configured reserve/limits;
- model/tier identity;
- persisted token usage and invocation history;
- operator overrides.

Controller judgment:

- whether another invocation has enough expected value;
- whether escalation is justified by previous outcomes;
- whether repeated identical attempts are wasteful;
- which eligible worker/tier best fits the current semantic problem.

`economy_escalation_exhausted` should eventually be an observed condition/reason for Controller recovery, not a policy dead-end.

## Repository-grounded behavioral evidence

The classification above is supported by current deterministic behavior/tests, not only module names:

- `src/operations.rs`: `ProjectOperations` is explicitly documented as a provider-independent, non-mutating read boundary; `TaskOperationsSummary` and `TaskOperationsDetail` already aggregate lifecycle, validation, review, blocker, execution-condition, economy and activity facts needed by a read-only Controller.
- `src/app.rs`: `OrcApp::requeue`, `unblock_non_convergence`, workflow entry points and Lead/plan methods show that canonical mutations and current policy are already composed at the application boundary rather than in the TUI/desktop.
- `src/scheduler.rs`: `schedule` and escalation-policy code demonstrate that deterministic eligibility and judgment-heavy escalation policy currently coexist and can be separated incrementally.
- `src/validation.rs`: `run_validation_pipeline` is the deterministic validation execution boundary.
- `src/lead.rs`: `PersistedLeadDecision` separates durable decision data from the Lead-specific reasoning role, which gives the migration a concrete persistence seam.
- `tests/dispatch_review_v2_tests.rs::failed_revision_keeps_initial_and_validation_repair_token_usage` documents revision/validation-repair lineage and accounting across failed repair attempts.
- `tests/dispatch_review_v2_tests.rs::dispatch_infrastructure_validation_failure_is_not_repair_non_convergence` documents the distinction between validation facts/infrastructure failure and semantic repair non-convergence.
- `tests/workflow_engine_tests.rs::non_convergence_cancellation_and_supersession_are_explicit` documents explicit workflow stop/recovery behavior rather than silent mutation.
- `src/agent.rs::non_convergence_recovery_rejects_invalid_attempts_without_mutation` documents that recovery legality is already guarded deterministically.

These tests are useful migration guards: Controller introduction should change who chooses a legal recovery action, not weaken persistence, validation truth, lineage or transition legality.

## Proposed read-only Controller state packet

M02 should build a bounded packet from existing read/storage surfaces rather than raw database access:

```text
ControllerStatePacket
├── project
│   ├── identity / repository state
│   └── current objective or plan context
├── task
│   ├── TaskOperationsSummary
│   ├── contract + dependencies
│   ├── execution_condition
│   └── legal_actions
├── execution
│   ├── current/latest run
│   ├── recent relevant runs only
│   └── current worktree/change evidence refs
├── validation
│   └── ValidationSummary + bounded failing diagnostics refs
├── review
│   ├── verdict/currentness
│   ├── actionable blockers
│   └── revision lineage/contract refs
├── economy
│   ├── latest resolution
│   ├── relevant escalation history
│   └── eligible agents + quota facts
├── recent_events
└── memory_refs          # added in M06; not raw memory dump
```

The packet should contain summaries and evidence references, not full transcripts, repository contents or complete historical logs. The Controller can request bounded detail through later typed read tools.

## Minimal integration seam

The smallest safe seam is a new read-only Controller module layered above existing application/read APIs:

```text
ProjectOperations + selected OrcApp read methods
                 ↓
       ControllerStateBuilder
                 ↓
       ControllerStatePacket
                 ↓
          LocalModelRuntime
                 ↓
     ControllerRecommendation
```

For the first Controller milestone there is **no mutation permission**. The recommendation is structured and inspectable. Existing CLI lifecycle commands continue to perform all mutations.

This avoids:

- exposing `Database` directly to the model;
- changing task lifecycle semantics;
- rewriting `OrcApp`;
- coupling Qwen/llama.cpp details to domain types;
- duplicating CLI/TUI/desktop logic.

## Native model boundary

M01 should introduce only the runtime seam required by the Controller. Model-specific prompt/tokenizer/GGUF/context/inference configuration stays behind the native runtime adapter. Controller state/action/domain types must not depend on Qwen-specific structures. Qwen3 8B + llama.cpp is the initial implementation choice, not a permanent domain dependency.

## Interface classification

- `src/main.rs` / `src/cli/`: operator adapter; should call canonical application APIs.
- `src/tui/`: interface adapter over shared `OrcApp` / `ProjectOperations` behavior.
- `src/desktop.rs`, `src-tauri/`, Vue UI: interface adapters; no Controller policy belongs here.

## Concrete M01/M02 direction

1. M01: add a native local-model runtime seam with Qwen3 8B/llama.cpp behind it. Do not connect it to lifecycle mutation.
2. M02: add `ControllerStatePacket` + builder from existing read surfaces and obtain a structured read-only recommendation from the local model.
3. Evaluate recommendations on known abnormal cases before granting typed mutation tools.

This is the first architecture checkpoint: if building M01/M02 requires broad lifecycle/storage rewrites, stop and revise this map rather than forcing the implementation.
