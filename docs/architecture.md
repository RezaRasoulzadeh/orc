# Architecture and application API

`src/main.rs` is the thin CLI boundary. `src/storage` owns SQLite access and migrations/compatibility checks. `src/task`, `src/queue`, `src/scheduler`, and `src/review` model work selection and lifecycle. `src/agent` and `src/worker` implement dispatch and worker backends. `src/lead` owns Lead context, provider invocation, and proposal state. `src/app.rs` is the application service used by the CLI, TUI, and Tauri commands. `src/tui` projects `ProjectOperations` into terminal state and routes explicit action keys back to `OrcApp`; it owns no storage or lifecycle rules. `src-tauri` serializes the same dashboard/task operational read models and routes explicit desktop actions to `OrcApp`; Vue does not compute lifecycle transitions, validation outcomes, Review verdicts, or economy policy. `src/lib/api.ts` describes that frontend contract.

Provider-specific execution is behind backend/worker abstractions. Project state is read from SQLite, and raw SQL stays in storage. Git integration creates and reviews task worktrees. Planning and Lead protocols exchange validated JSON and have explicit persistence boundaries.

The provider boundary and the Copilot CLI capability contract are documented in [provider contracts](providers.md).
