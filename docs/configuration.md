# Configuration

Project files live under `.orc/`: `orc.db` is authoritative state; `engineering.md` is the worker contract; discovery may create `project.md`, `architecture.md`, and `roadmap.md`; `worktrees/` contains isolated task checkouts.

Validation loads the first usable configuration in this order: `.orc/validation.toml`, `.orc/validation.json`, commands extracted from `.orc/engineering.md`, then the defaults `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.

Runtime timeouts can be set with `ORC_WORKER_TIMEOUT_SECS`, `ORC_LEAD_TIMEOUT_SECS`, and `ORC_VALIDATION_TIMEOUT_SECS`. Agent profile, model, reasoning effort, capabilities, availability, and quota are configured with `orc agent` commands. Credentials remain owned by the provider CLI and are not copied into SQLite.
