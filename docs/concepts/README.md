# Core concepts

An Orc project is one adopted Git repository. Tasks are durable units of work with an objective, role, priority, dependencies, required capabilities, scope, context files, and expected changes. Agents are registered worker identities with a backend, automated/manual mode, capabilities, priority, availability, and optional quota/model settings.

The scheduler computes eligibility from task status and dependencies, then evaluates enabled, available agents, capabilities, execution mode, priority, and quota reserve. It does not execute work. Dispatch creates a run; automated dispatch uses an isolated worktree, while manual dispatch waits for an operator or external worker.

The database is the source of truth. `status`, `queue`, `runs`, `report`, and the desktop dashboard are read models of that state.
