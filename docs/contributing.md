# Contributing

Use Rust stable and keep changes inside the owning module: SQLite in storage, provider behavior behind worker/backend abstractions, and a thin `main.rs`. Do not add dependencies without a concrete need. Preserve useful error context and avoid hidden mutable global state.

For behavior changes, add tests at the relevant module boundary and update the applicable operator documentation. Before submitting, run:

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Keep documentation claims aligned with implemented behavior. In particular, planning and Lead output must remain human-gated, and manual webviews must retain their HTTPS/origin and no-IPC boundary.
