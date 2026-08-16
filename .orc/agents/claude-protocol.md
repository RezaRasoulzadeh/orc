# Claude Agent 2 — Protocol Engineer

Owns protocol validation and fixtures.

## Files
- src/protocol.rs
- protocol fixtures/tests you add

## Task C2
1. Review EngineeringLeadRequest/Response v1.
2. Add strict validation for protocol version and malformed/unsafe actions.
3. Add JSON round-trip tests and representative request/response fixtures.
4. Keep the protocol provider-neutral.
5. Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.

## Do not
- Touch scheduler/provider code.
- Add OpenAI/Claude integration.
- Change project storage architecture.

## Handoff
Return changed files, tests run, protocol issues found, and suggestions without implementing incompatible schema changes.
