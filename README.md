# Orc

Orc v0.3.0 is a local, operator-controlled orchestrator for AI-assisted engineering. It keeps project, task, agent, run, approval, and lifecycle state in SQLite, uses Git worktrees to isolate dispatched work, and exposes the same state through a CLI and a Tauri desktop application.

Orc orchestrates external AI providers; it is not an AI model or provider. Provider credentials remain with the provider's own CLI or service. Orc does not silently plan, dispatch, apply patches, merge work, or mutate project state on an AI provider's behalf.

`agent remove` archives an agent and task cancellation preserves task history. `agent purge ID` and `task purge TASK_ID` are irreversible operations: agent purge removes the registered agent while preserving historical run attribution, and task purge removes task-owned state and, when safe, its canonical worktree. Active or waiting runs always prevent purge.

## Core concepts

An Orc project is an adopted Git repository. Tasks are durable units of work with an objective, role, priority, dependencies, required capabilities, scope, context files, and expected changes. Agents are registered workers with a backend, automated or manual execution mode, capabilities, priority, availability, and optional provider configuration.

Project-local state lives under `.orc/`. The authoritative database is `.orc/orc.db`; `.orc/engineering.md` is the worker contract; discovery can add project, architecture, and roadmap documents; and `.orc/worktrees/` contains isolated task checkouts. Keep the database and its SQLite WAL files together.

## Installation and initialization

Install Rust stable, Cargo, Git, Node.js, and the native Tauri prerequisites. From an Orc checkout, install the CLI and packaged desktop application with:

```bash
./scripts/install.sh
```

On Windows, run `powershell -ExecutionPolicy Bypass -File scripts/install.ps1`. `orc --ui` launches the installed packaged desktop application and returns immediately. See the [desktop installation guide](docs/desktop.md) for system-wide Linux installation, upgrades, uninstall steps, and artifact validation.

Run `orc init` to create local Orc state, then `orc adopt` to bring the existing Git repository under Orc management. The desktop Import action only remembers an already adopted project; it does not create project state. See [project lifecycle and ownership](docs/project-lifecycle.md) for the document, source-control, and runtime-state rules. Automated Codex workers and the configured Lead additionally require an installed and authenticated `codex` CLI. Manual workers and provider-independent acceptance tests do not require a live AI provider.

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

## CLI commands

The tables below cover every `orc` command. Flags are summarized; see the [complete CLI reference](docs/cli-reference.md) for every argument, default, and constraint, or run `orc <command> --help`.

### Project and health

| Command | Description |
|---|---|
| `orc init` | Initialize the Orc SQLite database in the current repository. |
| `orc adopt` | Adopt the current Git repository: record identity and write the engineering contract. |
| `orc discovery-request` | Emit a read-only repository discovery request as JSON. |
| `orc apply-discovery <PATH\|->` | Apply a structured discovery response to project state. |
| `orc doctor` | Diagnose project and agent health without consuming model quota. |
| `orc status` | Print project name and a one-line summary of every task. |
| `orc report [--full]` | Emit a structured project report JSON for a manual planner. |

### Planning and Engineering Lead

| Command | Description |
|---|---|
| `orc plan-request <OBJECTIVE> [--full-report]` | Emit a read-only planning request JSON for an objective. |
| `orc apply-plan <PATH\|->` | Validate and atomically apply a plan response, creating tasks. |
| `orc plan <OBJECTIVE> [--agent] [--model] [--effort]` | Run an automated planning action and print the plan response. |
| `orc ask <REQUEST> [--agent] [--model] [--effort]` | Address a request to the Engineering Lead. |
| `orc apply-response <PATH\|->` | Validate and persist an Engineering Lead response. |
| `orc lead show` | Show the configured Engineering Lead agent. |
| `orc lead set <AGENT> [--model] [--effort]` | Configure the Engineering Lead agent. |
| `orc lead clear` | Clear the configured Engineering Lead. |

### Tasks

| Command | Description |
|---|---|
| `orc task create <TITLE> <OBJECTIVE> [options]` | Create a task (`--role`, `--priority`, `--capability`, `--scope`, `--context`, `--expect`, `--depends-on`). |
| `orc task list` | List all tasks. |
| `orc task show <TASK_ID>` | Show full task detail. |
| `orc task require <TASK_ID> <CAPABILITY>...` | Set required capabilities for a task. |
| `orc task scope <TASK_ID> <MODE>` | Set a task's worktree scope mode. |
| `orc task context-add <TASK_ID> <PATH>...` | Add context file paths to a task. |
| `orc task context-clear <TASK_ID>` | Clear a task's context files. |
| `orc task expect-change <TASK_ID> <PATH>...` | Add expected-change paths to a task. |
| `orc task expect-clear <TASK_ID>` | Clear a task's expected changes. |
| `orc task diff <TASK_ID>` | Show the unified diff for a task's worktree. |
| `orc task worktree <TASK_ID>` | Show a task's worktree branch and path. |
| `orc task accept <TASK_ID>` | Integrate a reviewed task's branch and mark it done. |
| `orc task reject <TASK_ID> [REASON]` | Reject a reviewed task, preserving its worktree. |
| `orc task cancel <TASK_ID> [REASON]` | Cancel a task, preserving its worktree. |
| `orc task requeue <TASK_ID>` | Return an interrupted or failed task to the queue. |
| `orc task depend <TASK_ID> <DEPENDENCY_ID>` | Add a task dependency. |
| `orc task undepend <TASK_ID> <DEPENDENCY_ID>` | Remove a task dependency. |

### Queue and scheduling

| Command | Description |
|---|---|
| `orc queue [--explain]` | Show the deterministic task queue, optionally with readiness explanations. |
| `orc schedule <TASK_ID> [--explain] [--mode automated\|manual]` | Evaluate deterministic agent selection without dispatching. |

### Dispatch and runs

| Command | Description |
|---|---|
| `orc dispatch <TASK_ID> [--agent] [--model] [--effort]` | Dispatch a task using a selected agent. |
| `orc dispatch-queue [--concurrency N\|auto]` | Dispatch all ready automated tasks concurrently (default `1`). |
| `orc runs [TASK_ID]` | List agent runs, optionally filtered by task. |
| `orc run submit <RUN_ID> [--file PATH]` | Submit output for a waiting manual run. |
| `orc run submit-patch <RUN_ID> <PATCH_FILE\|->` | Submit and validate a Git patch for a manual run. |
| `orc run fail <RUN_ID> [REASON]` | Mark a manual run failed, moving its task to blocked. |

### Review and revision

| Command | Description |
|---|---|
| `orc review <TASK_ID> [--automated] [--agent] [--model] [--effort] [--diff \| --file PATH]` | Review a task contract, latest run, and worktree changes. |
| `orc project-review <TASK_ID> [--agent] [--model] [--effort]` | Run an unrestricted project-wide audit using captured task evidence. |
| `orc revise <TASK_ID> [FEEDBACK] [--agent] [--model] [--effort]` | Redispatch a reviewed task using optional feedback and execution overrides. |

### Agents

| Command | Description |
|---|---|
| `orc agents [--sync]` | List registered agents, optionally syncing quota first. |
| `orc agent list` | List registered agents. |
| `orc agent add <ID> --backend NAME [options]` | Register an agent (`--priority`, `--capability`, repeatable `--action` (`code`, `review`, `plan`, `lead`), `--display-name`, `--profile`, `--model`, `--effort`, `--mode`). |
| `orc agent onboard <ID> --backend NAME [options]` | Inspect provider login and capabilities, then persist only with `--approve`; supports repeatable `--permission` and `--role`. |
| `orc agent export <ID> [--output PATH]` | Export a versioned agent configuration without provider credentials. |
| `orc agent import PATH` / `update <ID> PATH` | Validate and atomically import or update a versioned global agent configuration. |
| `orc agent enable <ID>` / `disable <ID>` | Enable or disable an agent. |
| `orc agent remove <ID>` | Archive an agent. |
| `orc agent available <ID>` / `unavailable <ID> <REASON>` | Mark an agent available or unavailable. |
| `orc agent priority <ID> <PRIORITY>` | Set an agent's selection priority. |
| `orc agent profile <ID> <PATH>` | Set an agent's configuration profile directory. |
| `orc agent model <ID> <MODEL>` | Set an agent's model (automated Codex agents only). |
| `orc agent effort <ID> <EFFORT>` | Set an agent's reasoning effort (automated Codex agents only). |
| `orc agent quota <ID> --remaining N [--reset TS]` | Manually set an agent's quota state. |
| `orc agent quota-clear <ID>` | Clear an agent's manually set quota. |
| `orc agent quota-reserve <REMAINING>` | Set the global automatic-dispatch quota reserve. |
| `orc agent sync <ID>` | Synchronize quota through the provider's protocol. |
| `orc agent show <ID>` | Show full agent detail. |
| `orc agent actions <ID>` | Show supported actions and their profiles. |
| `orc agent action-add <ID> ACTION` | Add a supported action. |
| `orc agent action-remove <ID> ACTION` | Remove a supported action (the final action cannot be removed). |
| `orc agent permissions <ID>` / `permission-add` / `permission-remove` | Inspect or change operator-granted permissions independently from provider capabilities and Orc roles. |

### Execution templates

| Command | Description |
|---|---|
| `orc template list` | List the model/effort template for every execution class. |
| `orc template set <CLASS> [--model] [--effort]` | Set the persistent template for a class (`coder`, `reviewer`, `architect`, `researcher`, `general`). |
| `orc template clear <CLASS>` | Clear the persistent template for a class. |

### Approvals

| Command | Description |
|---|---|
| `orc approvals list` | List unresolved approval requests. |
| `orc approvals resolve <ID>` | Resolve an approval request. |

The [CLI reference](docs/cli-reference.md) documents every flag, default value, and constraint (e.g. which options require `--automated`, which conflict, and which are Codex-only) in full detail.

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

The v0.3.0 desktop workspace supports the complete normal operator lifecycle through controls and forms, with raw protocol JSON limited to advanced disclosures. Desktop development requires Node.js/npm and the platform prerequisites for Tauri.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
npm run typecheck
npm run build
```

See the [documentation index](docs/getting-started.md), [configuration guide](docs/configuration.md), [desktop guide](docs/desktop.md), and [CLI reference](docs/cli-reference.md) for further details.
