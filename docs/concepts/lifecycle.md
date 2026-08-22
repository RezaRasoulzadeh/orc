# Task lifecycle

Tasks begin in `backlog`. Dependency and scheduling rules can make them `ready`; dispatch moves work to `active`. A completed run moves the task to `review`. Review can lead to `done` through `orc task accept`, back to `ready` through reject/revise, or to `blocked` when work cannot proceed. Operators may move unfinished work to `cancelled`, or recover an interrupted active task with `orc task requeue`.

Acceptance integrates the task branch into the adopted repository. Rejection and cancellation preserve the worktree for inspection. Lifecycle events, run output, results, and timestamps remain queryable in SQLite.
