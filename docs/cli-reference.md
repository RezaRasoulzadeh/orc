# CLI reference

Run `orc --help` for the installed command surface. The main commands are:

- Project/state: `init`, `adopt`, `discovery-request`, `apply-discovery`, `status`, `report`, `doctor`.
- Planning/Lead: `plan-request`, `apply-plan`, `ask`, `apply-response`.
- Queue/dispatch: `queue`, `schedule`, `dispatch`, `dispatch-queue`, `runs`.
- Agents: `agents`, `agent list|add|show|enable|disable|available|unavailable|priority|profile|model|effort|quota|quota-clear|quota-reserve|sync`.
- Tasks: `task list|show|require|scope|context-add|context-clear|expect-change|expect-clear|depend|undepend|diff|worktree|accept|reject|cancel|requeue`.
- Manual runs/review: `run submit|submit-patch|fail`, `review`, `revise`.
- Approvals: `approvals list|resolve`.

Paths passed to response and output commands may be `-` where the command documents stdin support. Use `--explain` on queue and schedule to see the decision inputs.
