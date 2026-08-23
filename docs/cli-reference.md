# CLI reference

Run `orc --help` for the installed v0.2.2 command surface. Automated planning, review, and Lead responses are read-only until a human explicitly validates/applies or resolves the result; review never accepts or merges work. Malformed structured output fails without mutation.

- Project/state: `init`, `adopt`, `discovery-request`, `apply-discovery`, `status`, `report`, `doctor`.
- Planning/Lead: `plan-request`, `apply-plan`, `ask`, `apply-response`, `lead show|set|clear`.
- Queue/dispatch: `queue`, `schedule`, `dispatch`, `dispatch-queue`, `runs`.
- Agents: `agents`, `agent list|add|show|enable|disable|available|unavailable|priority|profile|model|effort|quota|quota-clear|quota-reserve|sync`.
- Tasks: `task list|show|require|scope|context-add|context-clear|expect-change|expect-clear|depend|undepend|diff|worktree|accept|reject|cancel|requeue`.
- Manual runs/review: `run submit|submit-patch|fail`, `review`, `revise`.
- Approvals: `approvals list|resolve`.

Paths passed to response and output commands may be `-` where the command documents stdin support. Use `--explain` on queue and schedule to see the decision inputs.
