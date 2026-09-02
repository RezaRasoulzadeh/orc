# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M01 — Native model runtime

**Current task:** M01-002 — Integrate llama.cpp behind the local runtime boundary

**Last completed:** M01-001 — Model-independent local inference runtime boundary

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one.
- Initial model/runtime target: Qwen3 8B through llama.cpp/GGUF.
- Qwen/llama.cpp is an implementation choice, not a domain dependency; the runtime boundary must remain replaceable.
- `ProjectOperations` is the primary existing provider-independent observation seam.
- `OrcApp` is the existing canonical application/mutation seam.
- `src/local_runtime.rs` is the replaceable, read-only local inference seam; native backend details remain outside Controller/domain types.
- Memory is explicit Orc data, separate from model weights.
- Lead/Planner reasoning will move into Controller after the Controller foundation is proven; durable useful Plan/decision data is preserved.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Execute M01-002. Integrate a pinned/reproducible native llama.cpp/GGUF adapter behind `LocalInferenceRuntime`, retain deterministic no-model tests, and add an opt-in real Qwen3 8B GGUF smoke path. Do not connect inference to Controller state, lifecycle or database mutation.

See `M00-REPOSITORY-MAP.md` for the repository-grounded migration map.
