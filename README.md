# Orc

Orc v0.2.2 is a local, operator-controlled orchestrator for AI-assisted engineering. It keeps project, task, agent, run, approval, and lifecycle state in SQLite, uses Git worktrees to isolate dispatched work, and exposes the same state through a CLI and a Tauri desktop application.

Orc orchestrates external AI providers; it is not an AI model or provider. Provider credentials remain with the provider's own CLI or service. Orc does not silently plan, dispatch, apply patches, merge work, or mutate project state on an AI provider's behalf.

## Core concepts

An Orc project is an adopted Git repository. Tasks are durable units of work with an objective, role, priority, dependencies, required capabilities, scope, context files, and expected changes. Agents are registered workers with a backend, automated or manual execution mode, capabilities, priority, availability, and optional provider configuration.

Project-local state lives under `.orc/`. The authoritative database is `.orc/orc.db`; `.orc/engineering.md` is the worker contract; discovery can add project, architecture, and roadmap documents; and `.orc/worktrees/` contains isolated task checkouts. Keep the database and its SQLite WAL files together.

## Installation and initialization

Install Rust stable, Cargo, and Git. From an Orc checkout, install the CLI with:

```bash
cargo install --path .
```

Run `orc init` in the repository Orc should manage, then `orc adopt` to record its Git identity and engineering contract. Automated Codex workers and the configured Lead additionally require an installed and authenticated `codex` CLI. Manual workers and provider-independent acceptance tests do not require a live AI provider.

## Verified quick start

From a Git repository, the smallest useful inspection workflow is:

```bash
orc init
orc adopt
orc status
orc task create "Document the project" "Write a concise project overview" --role developer
orc task list
orc queue --explain
```

Register an agent with `orc agent add`, inspect eligibility with `orc schedule TASK_ID --explain`, and dispatch with `orc dispatch TASK_ID` or `orc dispatch-queue --concurrency 2`. Use `orc --help` and the [complete CLI reference](docs/cli-reference.md) for exact options and command-specific usage.

## Command families

- Project and health: `init`, `adopt`, `discovery-request`, `apply-discovery`, `status`, `report`, `doctor`.
- Planning and Lead: `plan-request`, `apply-plan`, `plan`, `ask`, `apply-response`, and `lead show|set|clear`.
- Tasks and queue: `task ...`, `queue`, and deterministic `schedule`.
- Agents and configuration: `agents` and `agent ...` for registration, enablement, availability, capabilities, profiles, model, effort, priority, and quota.
- Execution: `dispatch`, `dispatch-queue`, `runs`, and `run submit|submit-patch|fail`.
- Review and lifecycle: `review`, `revise`, `task diff|worktree|accept|reject|cancel|requeue`.
- Approvals: `approvals list|resolve`.
- Persistent execution templates: `template list|set|clear`.

The [CLI reference](docs/cli-reference.md) is exhaustive; this overview intentionally omits the full option list.

## Agents, profiles, models, and effort

Agents can be automated or manual. Automated workers execute through supported backends in isolated worktrees; manual agents receive a task packet and wait for submitted output or a validated patch. A registered agent may have a provider profile directory, model, reasoning effort (`none`, `low`, `medium`, or `high`), capabilities, availability, and quota settings.

v0.2.2 persists action-specific profiles for code, review, planning, and Lead actions. A single automated agent can therefore resolve different model and effort settings per action. Explicit per-command overrides take precedence and execution records the resolved action, agent, settings, status, token usage, and result. The desktop exposes these settings; the CLI provides the agent, Lead, and per-run configuration commands documented in the CLI reference.

Execution templates persist in SQLite for `coder`, `reviewer`, `architect`, `researcher`, and `general`. Resolution is deterministic: per-run override, persistent class template, compatible environment template, the coder low-effort compatibility default, agent configuration, then provider default. See [configuration](docs/configuration.md) for environment variables and timeouts.

## Tasks, queue, and dispatch

Create tasks directly with `orc task create`, or apply a reviewed structured plan. Dependencies and required capabilities determine readiness. `orc queue --explain` reports the deterministic queue and explains dependency, readiness, and scheduler eligibility decisions. `orc schedule TASK_ID --explain` evaluates candidates using enabled and available agents, execution mode, capabilities, priority, and quota reserve.

Dispatch a selected task with `orc dispatch TASK_ID`, or dispatch ready automated tasks with bounded concurrency using `orc dispatch-queue --concurrency 2` (the default is `1`; `auto` is supported). Scheduling and dispatch are explicit operator actions.

## Runs, review, acceptance, and recovery

Automated runs work in task-specific Git worktrees. Manual runs print a task packet and wait for `orc run submit RUN_ID`, `orc run submit-patch RUN_ID PATCH_FILE`, or `orc run fail RUN_ID`. Inspect runs with `orc runs`; inspect changes with `orc review TASK_ID`, `orc task diff TASK_ID`, and `orc task worktree TASK_ID`.

Review does not accept or merge work. Send feedback with `orc revise TASK_ID "feedback"`; integrate satisfactory work with `orc task accept TASK_ID`; reject it with `orc task reject TASK_ID "reason"` while preserving the worktree; or cancel unfinished work with `orc task cancel TASK_ID "reason"`. An interrupted active task or failed blocked task can return to the queue with `orc task requeue TASK_ID`.

## Planning, Lead, review, and approvals

`orc report` and `orc report --full` provide structured project state for a manual planner. `orc plan-request OBJECTIVE` emits a read-only `PlanningRequest`; a human reviews the returned `PlanResponse` and explicitly applies it with `orc apply-plan FILE`. The Engineering Lead protocol works similarly: `orc ask REQUEST` emits an `EngineeringLeadRequest`, and `orc apply-response FILE` validates and persists the response. `orc plan` and `orc review --automated` can run supported automated actions, but their results still follow the review and approval boundaries.

Planner and Lead exchanges use structured, versioned protocols. Invalid versions, actions, plans, or responses fail without mutating state. Proposals that require approval are listed with `orc approvals list` and resolved explicitly with `orc approvals resolve APPROVAL_ID`.

## Desktop project registry

The Tauri desktop application shares the CLI's SQLite project state and provides dashboard, queue, tasks, agents, runs, review, planning, Lead, approvals, and manual-run views. Its persistent project registry supports registering projects, switching between them, detecting missing or moved projects, and relocating a registered project. Removing a project removes only its registry entry; re-importing it preserves the existing `.orc/orc.db`. A project must already be initialized and adopted before it can be opened.

This v0.2.2 desktop architecture supports persistent projects and sessions at the registry level. The major UI redesign is planned for v0.3. Desktop development requires Node.js/npm and the platform prerequisites for Tauri.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
npm run typecheck
npm run build
```

See the [documentation index](docs/getting-started.md), [configuration guide](docs/configuration.md), [desktop guide](docs/desktop.md), and [CLI reference](docs/cli-reference.md) for further details.
