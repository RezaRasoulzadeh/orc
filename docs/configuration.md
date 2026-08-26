# Configuration

Project lifecycle and ownership are defined in [Project lifecycle and ownership](project-lifecycle.md). In brief, committed project documents live under `.orc/`, while `orc.db` and its WAL files are authoritative but untracked Orc runtime state; `worktrees/` contains untracked isolated task checkouts. `.orc/engineering.md` is project-owned and is automatically loaded by coder execution.

Validation loads the first usable configuration in this order: `.orc/validation.toml`, `.orc/validation.json`, commands extracted from `.orc/engineering.md`, then the defaults `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.

Runtime timeouts can be set with `ORC_WORKER_TIMEOUT_SECS`, `ORC_LEAD_TIMEOUT_SECS`, and `ORC_VALIDATION_TIMEOUT_SECS`. Agent profile, model, reasoning effort, capabilities, availability, and quota are configured with `orc agent` commands. Credentials remain owned by the provider CLI and are not copied into SQLite.

Lead execution is persisted in SQLite. Use `orc lead show`, `orc lead set <agent> [--model <model>] [--effort <none|low|medium|high>]`, or `orc lead clear`. The selected agent must be an enabled automated Codex agent. v0.2.2 persists distinct code, review, plan, and lead profiles for one automated agent; explicit overrides take precedence and each action records its action, agent, resolved settings, status, token usage, and result.

Execution templates are persistent SQLite configuration for the `coder`, `reviewer`, `architect`, `researcher`, and `general` classes. Inspect, set, or reset them with `orc template list`, `orc template set <class> --model <model> --effort <none|low|medium|high>`, and `orc template clear <class>`. Resolution is deterministic: per-run overrides, persistent class template, compatible environment template, the coder low-effort compatibility default, agent configuration, then the provider default. Environment variables use `ORC_CODER_*`, `ORC_REVIEW_*` (reviewer and architect), `ORC_RESEARCH_*`, and `ORC_GENERAL_*`.
