# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M03 — Typed Controller tools

**Current task:** M03-002 — Execute explicitly authorized Controller intents (Planned)

**Last completed:** M03-001 — Define typed Controller action intents and legality boundary

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one.
- Initial model/runtime target: Qwen3 8B through llama.cpp/GGUF.
- Qwen/llama.cpp is an implementation choice, not a domain dependency; the runtime boundary must remain replaceable.
- `ProjectOperations` is the primary existing provider-independent observation seam.
- `OrcApp` is the existing canonical application/mutation seam.
- `src/local_runtime.rs` is the replaceable, read-only local inference seam; native backend details remain outside Controller/domain types. The optional `llama-cpp` feature implements the first adapter in `src/local_runtime/llama_cpp.rs`.
- Controller recommendations use a model-independent JSON Schema request; the llama.cpp adapter applies native grammar-constrained sampling and strict full-value parsing, retaining raw output on structured parse failure.
- Controller state carries an explicit canonical operational-consistency observation so derived next-step data cannot silently override contradictory lifecycle facts.
- M02 is complete: the final unchanged seven-scenario Qwen3 evaluation achieved `7/7` semantic decisions and `7/7` strict structured-contract compliance.
- M03-001 is complete and source-reviewed `PASS`: typed Controller intents for dispatch/review/revise/accept can be inspected through canonical `ProjectOperations` legality without mutation.
- A legality inspection is not authorization and is not a durable grant. Mutation-capable Controller execution must receive explicit trusted authorization and re-check legality immediately before mutation.
- Model-owned intent must never carry or manufacture its own authorization/confirmation.
- Memory is explicit Orc data, separate from model weights.
- Lead/Planner reasoning will move into Controller after the Controller foundation is proven; durable useful Plan/decision data is preserved.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement M03-002 exactly against `docs/orc-next/tasks/M03-002.md`.

M03-002 adds the first mutation-capable Controller boundary, but only for the
four M03-001 intents. A trusted Orc caller must explicitly authorize one
execution; the boundary then performs a fresh
`ProjectOperations::inspect_action` check immediately before mutation and
forwards an allowed request to the existing canonical Orc action path.

Do not treat a previous `Allowed` inspection as permission. Do not put approval,
agent/economy policy, arbitrary commands, provider payloads, SQL, paths, or
runtime handles into the model-owned intent. Keep worker/action configuration in
trusted existing Orc configuration/override seams.

This remains a supervised execution slice, not autonomy: no continuation loop,
recovery actions, planning/Lead migration, memory, scheduler redesign, model
changes, Python, GPU work, or new interface confirmation UX.

M03-001 completion evidence:

- Luna + High source review: `PASS`;
- focused action tests: 7 passed;
- `cargo test --lib`: 300 passed;
- `cargo test --features llama-cpp --lib`: 306 passed;
- normal and feature clippy, fmt, and diff check: passed.

See `M00-REPOSITORY-MAP.md` for the repository-grounded migration map.
