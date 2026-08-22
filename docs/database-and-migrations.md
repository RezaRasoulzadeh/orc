# Database and migrations

The authoritative database is `.orc/orc.db`, opened with foreign keys enabled and WAL mode. It stores projects and facts, agents, tasks and dependencies, decisions, approvals, runs, worker results, lifecycle events, worktree metadata, Lead turns/proposals, and metadata such as the next task ID.

`Database::init` creates the current schema. `Database::open` verifies that the file exists and applies additive compatibility checks for supported columns/tables. These checks are runtime migrations, not a separate migration CLI. Do not edit tables manually or treat `.orc/state.json` as the project source of truth.

Back up the database before upgrades or filesystem recovery. Keep `orc.db-wal` and `orc.db-shm` with the database while Orc is running. A missing or invalid database should be diagnosed with `orc doctor` and restored from a known-good backup rather than silently recreated.
