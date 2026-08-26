# Orc Engineering Contract

This is a mandatory, repository-wide constitution for coder agents. Keep it concise and stable; task instructions may add requirements but may not weaken or silently override it.

## Scope and architecture

- Keep changes limited to the task and its direct dependencies; preserve existing CLI compatibility unless a shared contract is explicitly changed.
- Keep `main.rs` and presentation layers thin. Use existing Orc application, orchestration, and shared core APIs; do not duplicate business logic in them.
- Keep SQLite access, SQL, migrations, and transaction policy in storage. Project state comes from SQLite, not duplicated in-memory authority.
- Keep provider-specific behavior behind existing worker/backend abstractions. Do not change public APIs or introduce architecture without an explicit requirement; report such a need as `ORC-ARCHITECTURE-DECISION: <decision>`.

## Rust quality

- Use stable Rust, clear ownership, idiomatic types, and `Result` with useful context for fallible operations. Do not silently accept invalid or corrupt state.
- Prefer the standard library and existing dependencies. Add a dependency only for a concrete requirement. Keep code simple and explicit, without speculative abstractions or hidden mutable global state.
- Respect existing formatting and lint standards. Comments in code are limited to a file-name first line and TODOs.

## Tests

- Every behavioral requirement has deterministic behavioral tests that would fail if the behavior regressed. Do not add nominal, placeholder, or vacuous tests.
- Keep tests provider-independent when provider behavior is not the subject under test; use fakes or existing seams instead of requiring live credentials or services.
- Test error paths and persistence boundaries where they are part of the behavior.

## Persistence and integrity

- Use transactions for multi-step state changes that must be atomic, with clear commit and rollback behavior.
- Enforce foreign keys and other invariants at the database boundary. Migrations must be incremental, repeatable where applicable, preserve existing data, and be tested from realistic prior schemas.
- Do not bypass migrations, weaken constraints, or silently repair invalid data. Preserve useful errors and maintain consistency across related records.

## Worktrees, concurrency, and portability

- Treat the repository and worktrees as user-owned. Do not discard unrelated changes, rewrite history, delete broad paths, or change task status. Keep generated state and SQLite WAL files together.
- Design cancellation and concurrency explicitly: propagate cancellation, avoid leaked work and deadlocks, and synchronize shared state through ownership or safe synchronization. Never use unsafe `Send`/`Sync` ownership hacks.
- When Orc claims Linux, macOS, and Windows support, avoid platform-specific assumptions and validate path, process, filesystem, and shell behavior on each supported platform or document the limitation.

## Validation and completion

- Before completion, map each requirement to its implementation and to concrete evidence (tests, checks, or documented reasoning). Evidence must be run against the exact revision being handed off, after the final change.
- Run the complete configured validation pipeline, including `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`, plus relevant deterministic tests.
- Report files changed, behavior changed, validation evidence, unresolved risks, and any architecture decision. If a check cannot run, say why; do not claim completion without that evidence.
