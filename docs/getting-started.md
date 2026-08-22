# Getting started

## Prerequisites

Install Rust stable and Git. Build Orc with `cargo install --path .` or run it from the checkout with `cargo run --`. Automated Codex execution and the configured Codex Lead require the `codex` command and its own credentials/profile. Desktop builds require Node.js/npm and the Tauri prerequisites for the host platform.

## Initialize a repository

Run these commands from the Git repository Orc should manage:

```text
orc init
orc adopt
orc doctor
```

`init` creates `.orc/orc.db`. `adopt` records repository metadata and the engineering contract. Keep `.orc/orc.db` and its WAL files together; `.orc/worktrees/` contains task worktrees.

## First task

Use `orc plan-request "..."` to emit a read-only planning request. Review the JSON, then apply it with `orc apply-plan response.json`. Alternatively use `orc ask "..."`; Lead output is a proposal and becomes state only after `orc apply-response response.json` or an explicit desktop Apply action.

Inspect with `orc status`, `orc task list`, and `orc queue --explain`. Register an agent, schedule or dispatch a task, inspect the run, review the diff, and accept it only after human review:

```text
orc agent add local-codex --backend codex --profile PATH
orc dispatch T-0001
orc review T-0001 --diff
orc task accept T-0001
```

For a manual worker, use `--mode manual`, dispatch, then submit output or a patch with the run commands described in [manual workflow](workflows/manual-agent.md).

## Documentation map

- [Concepts](concepts/README.md), [lifecycle](concepts/lifecycle.md), and [Lead/planning/approvals](concepts/lead-and-approvals.md)
- [Automated workflow](workflows/automated.md), [manual workflow](workflows/manual-agent.md), and [recovery](workflows/recovery.md)
- [Configuration](configuration.md) and [CLI reference](cli-reference.md)
- [Desktop application](desktop.md)
- [Architecture and application API](architecture.md), [database and migrations](database-and-migrations.md)
- [Troubleshooting](troubleshooting.md) and [contributing](contributing.md)
