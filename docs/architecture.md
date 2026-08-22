# Architecture and application API

`src/main.rs` is the thin CLI boundary. `src/storage` owns SQLite access and migrations/compatibility checks. `src/task`, `src/queue`, `src/scheduler`, and `src/review` model work selection and lifecycle. `src/agent` and `src/worker` implement dispatch and worker backends. `src/lead` owns Lead context, provider invocation, and proposal state. `src/app.rs` is the application service used by both CLI and Tauri commands. `src-tauri` exposes read models and explicit actions; `src/lib/api.ts` describes the frontend contract.

Provider-specific execution is behind backend/worker abstractions. Project state is read from SQLite, and raw SQL stays in storage. Git integration creates and reviews task worktrees. Planning and Lead protocols exchange validated JSON and have explicit persistence boundaries.
