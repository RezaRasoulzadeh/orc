# Orc

Orc is a local, operator-controlled control plane for AI-assisted engineering. It stores project, task, agent, run, approval, and lifecycle state in SQLite and uses Git worktrees to isolate dispatched work. A Tauri desktop application presents the same project state and actions as the CLI.

Orc v0.2 is a local developer tool. It does not silently plan, dispatch, apply patches, or mutate project state on behalf of an AI provider.

## Install and five-minute workflow

Prerequisites: Rust stable with Cargo and Git. Automated Codex workers and the Lead additionally require an installed and authenticated `codex` CLI; the manual-agent workflow and the acceptance suite require no live AI provider. Desktop development additionally needs Node.js/npm and the Tauri platform prerequisites.

Build from a checkout:

```bash
cargo install --path .
orc init
orc adopt
orc discovery-request > /tmp/orc-discovery.json
orc status
orc queue --explain
```

`orc init` creates `.orc/orc.db`; `orc adopt` records the current Git repository and writes the engineering contract used by workers. Discovery, planning, dispatch, review, and acceptance are separate operator actions. Start with [`docs/getting-started.md`](docs/getting-started.md).

## What Orc provides

- Persistent projects, tasks, dependencies, agents, runs, approvals, and lifecycle events.
- Deterministic readiness and scheduling with capabilities, priority, availability, execution mode, and quota reserve.
- Automated workers in isolated Git worktrees and manual-agent task packets or validated patches.
- Review, revision, rejection, acceptance, cancellation, and recovery operations.
- Read-only discovery/planning protocols and a human-gated Engineering Lead proposal workflow.
- CLI and Tauri desktop views over the same SQLite-backed application state.

See the [documentation index](docs/getting-started.md) for the detailed operator, architecture, and contributor guides.

## Planning and task lifecycle

Inspect the current project for a manual planner with `orc report` or `orc report --full`. Create a read-only planning request with `orc plan-request "Describe the next engineering increment"` or `orc plan-request --full-report "..."`. A human reviews the returned `PlanResponse` JSON, then applies it explicitly with `orc apply-plan plan-response.json`.

The Engineering Lead protocol is also available: `orc ask "..."` emits an `EngineeringLeadRequest`, and `orc apply-response RESPONSE.json` validates and persists an `EngineeringLeadResponse`. Neither protocol dispatches work implicitly.

Tasks move through backlog/ready, active, review, done, blocked, or cancelled states. Inspect them with `orc status`, `orc task list`, and `orc task show TASK_ID`. Configure dependencies, capabilities, scope, context, and expected changes with `orc task depend`, `orc task undepend`, `orc task require`, `orc task scope`, `orc task context-add`, `orc task expect-change`, and their clear commands.

## Queue, agents, and scheduling

`orc queue` shows the deterministic queue; `orc queue --explain` includes dependency, readiness, and scheduler eligibility details. Register and maintain workers with `orc agent add`, `orc agent list`, `orc agent show`, `orc agent enable`, `orc agent disable`, `orc agent available`, and `orc agent unavailable`. Agents have a backend, execution mode (`automated` or `manual`), priority, capabilities, and optional Codex model/reasoning configuration.

Use `orc schedule TASK_ID --explain` to see candidate evaluation, or `orc schedule TASK_ID` to select an eligible agent. Dispatch automated work with `orc dispatch TASK_ID` or ready tasks with `orc dispatch-queue --concurrency 2` (default: 1; `auto` is supported). Quota can be inspected or synchronized with `orc agents --sync` and `orc agent sync AGENT_ID`, and configured with the agent quota commands.

## Manual workers

Dispatch to a manual agent with `orc dispatch TASK_ID --agent AGENT_ID`. Orc prints a task packet and records a waiting external run. Submit worker output with `orc run submit RUN_ID --file RESPONSE.txt`, or submit a Git patch with `orc run submit-patch RUN_ID PATCH_FILE`. Use `-` for stdin. Record failure with `orc run fail RUN_ID "reason"`.

## Review, revision, and acceptance

Completed runs enter review. Inspect with `orc runs`, `orc runs TASK_ID`, `orc review TASK_ID`, `orc task diff TASK_ID`, and `orc task worktree TASK_ID`. `orc review TASK_ID --diff` includes the complete diff; `--file PATH` limits it to one changed file.

Send feedback with `orc revise TASK_ID "feedback"`. Integrate satisfactory work with `orc task accept TASK_ID`. Preserve the worktree and return the task to ready with `orc task reject TASK_ID "reason"`. Cancel unfinished work with `orc task cancel TASK_ID "reason"`.

## Approvals, recovery, and observability

Architecture or other worker decisions requiring approval are listed with `orc approvals list` and resolved with `orc approvals resolve APPROVAL_ID`. After an interrupted process, inspect `orc runs` and recover an interrupted task with `orc task requeue TASK_ID`.

`orc status` gives a compact project/task view, `orc queue --explain` explains scheduler state, `orc runs` shows run status, phase, elapsed time, activity, output, and timestamps, `orc report` emits structured project state, and `orc doctor` checks operational health. Orc preserves this state in SQLite and does not silently discard invalid protocol responses or corrupted state.

## v0.2.2 release gate

The provider-independent v0.2.2 acceptance coverage exercises project registration, switching, relocation, re-import persistence, manifest-relative startup, human-gated planning/review/Lead actions, malformed-output rejection, action profiles, explicit overrides, and persisted execution metadata. It uses test doubles and manual runs, so it does not require a live AI provider. The v0.3 visual redesign is not part of this release.
