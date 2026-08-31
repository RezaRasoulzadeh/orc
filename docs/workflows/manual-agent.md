# Manual-agent workflow

Register an agent with `--mode manual`, then dispatch it. Orc creates a task worktree and waiting external run, and prints both the worktree path and task packet. Give that packet to the human or external provider. The worker implements in that worktree and returns the packet's structured completion JSON with `orc run submit RUN_ID --file OUTPUT`; alternatively, submit a Git patch with `orc run submit-patch RUN_ID PATCH` (use `-` for stdin).

Both submission commands derive authoritative changes from the actual task worktree and run Orc-owned deterministic validation before the task becomes Review-ready. Worker-reported affected files and validation claims are descriptive only. An ordinary validation failure records evidence and blocks the manual run for operator correction; Orc does not fabricate an automated repair worker. Infrastructure failures are recorded separately and also prevent Review readiness. Review is semantic-only and requires fresh passing validation evidence for the current worktree. Record an unsuccessful attempt with `orc run fail RUN_ID "reason"`.

The desktop application exposes the same waiting runs and actions. A configured manual provider may also be opened in an embedded HTTPS webview; see [desktop security boundaries](../desktop.md#manual-provider-webviews).
