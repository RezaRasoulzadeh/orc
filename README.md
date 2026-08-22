# Orc

Orc is a local Rust CLI for coordinating AI-assisted engineering work. It keeps project, task, agent, run, approval, and lifecycle state in `.orc/orc.db`, and uses Git worktrees to isolate dispatched task changes.

## Install and first project

Orc currently runs from a Rust checkout:

```bash
cargo install --path .
orc init
orc adopt
orc discovery-request > /tmp/orc-discovery.json
orc status
orc queue --explain
```

`orc init` creates local SQLite state and a project record. Run it from the repository Orc should manage. `orc adopt` records the current Git repository, and `discovery-request` emits a read-only JSON request. Apply a discovery response with `orc apply-discovery RESPONSE.json` (or `-` for stdin). `orc doctor` reports project, agent, and active-task health.

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
