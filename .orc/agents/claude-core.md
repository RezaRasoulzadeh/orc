# Claude Agent 1 — Core Engineer

Owns task/state persistence and CLI correctness.

## Files
- src/task.rs
- src/state.rs
- CLI task/state portions of src/main.rs

## Task C1
1. Review current skeleton for Rust compile errors and API problems.
2. Make `orc init`, `orc status`, and `orc task list` robust.
3. Add task creation/application persistence needed by EngineeringLeadResponse.
4. Add focused unit tests for state/task transitions.
5. Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.

## Do not
- Redesign the Engineering Lead protocol.
- Add AI/provider integrations.
- Add a database yet.

## Handoff
Return changed files, tests run, failures/risks, and any proposed protocol change separately.
