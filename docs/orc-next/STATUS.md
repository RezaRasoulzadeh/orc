# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M01 — Native model runtime

**Current task:** M01-001 — Introduce the model-independent local runtime boundary

**Last completed:** M00-001 — Current repository mapped into kernel, judgment/policy, interface and migration surfaces

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one.
- Initial model/runtime target: Qwen3 8B through llama.cpp/GGUF.
- Qwen/llama.cpp is an implementation choice, not a domain dependency; the runtime boundary must remain replaceable.
- `ProjectOperations` is the primary existing provider-independent observation seam.
- `OrcApp` is the existing canonical application/mutation seam.
- Memory is explicit Orc data, separate from model weights.
- Lead/Planner reasoning will move into Controller after the Controller foundation is proven; durable useful Plan/decision data is preserved.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Execute M01-001. Inspect the current Rust build/dependency/runtime patterns and implement the smallest model-independent local inference boundary with deterministic fake-runtime tests. Do not connect the model to lifecycle/database mutation and do not build Controller state packets yet.

See `M00-REPOSITORY-MAP.md` for the repository-grounded migration map.
