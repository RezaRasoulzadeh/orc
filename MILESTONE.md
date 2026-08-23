# Orc v0.2 — Local orchestration workflow and operator documentation

## Goal

Provide a persistent, operator-controlled loop for planning, scheduling, executing, reviewing, and accepting AI-assisted engineering tasks in a local Git repository, with documentation that describes only the implemented CLI, desktop, storage, and provider boundaries.

The v0.2 documentation set is indexed from [`docs/getting-started.md`](docs/getting-started.md). It covers concepts, automated and manual workflows, recovery, configuration, CLI, desktop usage, architecture/API, database behavior, troubleshooting, and contributing.

## Supported workflow

1. Initialize with `orc init`, adopt with `orc adopt`, and optionally exchange read-only discovery requests and responses.
2. Inspect with `orc report` or `orc report --full`. A planner receives `orc plan-request OBJECTIVE` and returns a validated `PlanResponse`; a human applies it with `orc apply-plan FILE`.
3. Inspect readiness with `orc status`, `orc task list`, and `orc queue --explain`. Dependencies and required capabilities determine eligibility.
4. Register agents, then use deterministic `orc schedule TASK_ID` selection or dispatch directly. `orc dispatch-queue` runs ready automated tasks with bounded concurrency.
5. Automated workers execute in isolated Git worktrees. Manual workers receive a packet and complete through `orc run submit`, `orc run submit-patch`, or `orc run fail`.
6. Completed work enters review. Operators use `orc review`, `orc task diff`, and `orc runs`, then revise with `orc revise`, reject with `orc task reject`, or integrate with `orc task accept`.
7. Interruptions are recoverable with `orc task requeue`. Approval requests are inspected and resolved with `orc approvals list` and `orc approvals resolve`.

## State and behavior

- SQLite is the source of project, task, agent, run, approval, and lifecycle state.
- Queue output is deterministic and explains backlog, dependencies, readiness, active work, review, blocked work, completion, and cancellation.
- Scheduling considers enabled/available agents, execution mode, capabilities, priority, and configured quota reserve.
- Automated runs use supported worker backends and validation. Manual runs wait for an operator or external worker and can accept validated patches.
- Review preserves worktree and run information. Acceptance integrates the task branch; rejection returns the task to ready while preserving the worktree.
- Invalid JSON protocol versions, actions, plans, and responses fail with useful errors rather than mutating state.
- `orc status`, `orc queue --explain`, `orc runs`, `orc report`, and `orc doctor` provide operator visibility.

## Protocols

The planner protocol is `PlanRequest`/`PlanResponse`: `orc plan-request` is read-only, and `orc apply-plan` is the explicit persistence boundary after human review. The Engineering Lead protocol is `EngineeringLeadRequest`/`EngineeringLeadResponse`: `orc ask` emits a request and `orc apply-response` persists a validated response.

Both protocols are structured JSON interfaces. Orc does not include a built-in AI provider, silently dispatch work, or apply a plan without an explicit operator command.

## v0.2 freeze gate

- A repository can be initialized and adopted, with project state persisted in `.orc/orc.db`.
- Operators can plan or request task changes, validate and apply structured responses, and inspect the resulting lifecycle.
- Ready work can be queued, scheduled, dispatched automatically or manually, recovered, reviewed, revised, rejected, accepted, or cancelled.
- Approvals, quotas, worker runs, worktrees, validation, and health checks are observable through the CLI.
- The provider-independent acceptance suite covers a clean temporary project, direct task creation, agent registration and safe lifecycle behavior, persistent execution-template resolution, deterministic queue explanation, manual dispatch, failed-run recovery, review/acceptance, persistent Lead configuration and human-gated proposals, approvals, reopen persistence, and operator-facing formatting invariants.
- No acceptance path requires a live external AI provider.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `npm run typecheck`, and `npm run build` pass.

The v0.2 release gate includes `tests/v02_acceptance_tests.rs`; after it passes, v0.2 core behavior is frozen and subsequent desktop redesign belongs to v0.3. The earlier `tests/v01_acceptance_tests.rs` remains historical coverage for initialization, adoption, manual task packet delivery, validation, review, Git integration, and doctor output.
