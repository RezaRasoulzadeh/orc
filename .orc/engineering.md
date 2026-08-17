# Orc Engineering Contract

## General
- Rust stable.
- Prefer standard library and small dependencies.
- Keep implementations simple and explicit.
- Do not introduce abstractions without a current requirement.
- Do not modify unrelated modules.

## Architecture
- main.rs must remain thin.
- SQLite logic belongs in storage.
- AI/provider-specific logic belongs behind worker/backend abstractions.
- Project state must come from SQLite.
- No raw SQL outside storage.
- No hidden global mutable state.

## Error handling
- Use Result for fallible operations.
- Do not silently recover from corrupted or invalid state.
- Errors must preserve useful context.

## Dependencies
- New dependencies require a concrete reason.
- Prefer existing dependencies when appropriate.
- Do not add frameworks for small problems.

## AI workers
- Workers must follow existing architecture.
- Workers must not introduce new architectural patterns without approval.
- Workers must not change public APIs unless the task explicitly requires it.
- Workers must stay inside their assigned task scope.
- If architecture must change, stop and report it instead of implementing it.

## Tests and validation
Every implementation must pass:

cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test

New public behavior must have tests.

## Completion
A worker must report:
- files changed
- behavior changed
- tests/checks run
- unresolved risks
- any architectural decision it believes is required
