# Troubleshooting

Start with `orc doctor`, `orc status`, `orc queue --explain`, and `orc runs`.

- “run `orc init` first”: run commands from the adopted repository and confirm `.orc/orc.db` exists.
- No eligible agent: check enabled/available status, capabilities, execution mode, dependencies, and quota reserve with `agent show` and `schedule --explain`.
- Codex/profile errors: verify the provider CLI, profile path, authentication, and backend mode; Orc does not store provider credentials.
- Stalled active task: inspect its run and use `orc task requeue TASK_ID` only when the run is interrupted.
- Review/accept errors: inspect the preserved worktree and repository cleanliness; resolve conflicts or invalid changes before retrying.
- Manual webview unavailable: use an absolute HTTPS workspace URL on the configured provider host, or use task packets and run submission.
- Malformed JSON or database errors: preserve the exact error and input, restore a backup if necessary, and do not delete state as a first response.
