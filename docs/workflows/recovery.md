# Recovery

After an interrupted process, inspect `orc runs`, `orc status`, `orc queue --explain`, and `orc doctor`. If a task has a recoverable non-terminal run, `orc task requeue TASK_ID` records recovery and returns it to the queue. Do not delete the database or worktree while diagnosing. Review preserved work with `orc task worktree` and `orc task diff`.

Invalid protocol JSON, missing projects, invalid task transitions, and corrupted state are errors; Orc does not silently repair them. Back up `.orc/orc.db` before filesystem-level recovery.
