# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M02 — Read-only Controller

**Current task:** M02-001 — Build the read-only Controller state/recommendation path (Ready)

**Last completed:** M01-002 — Pinned native llama.cpp adapter and opt-in smoke path (source review PASS; Qwen3 GGUF smoke PASS)

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one.
- Initial model/runtime target: Qwen3 8B through llama.cpp/GGUF.
- Qwen/llama.cpp is an implementation choice, not a domain dependency; the runtime boundary must remain replaceable.
- `ProjectOperations` is the primary existing provider-independent observation seam.
- `OrcApp` is the existing canonical application/mutation seam.
- `src/local_runtime.rs` is the replaceable, read-only local inference seam; native backend details remain outside Controller/domain types. The optional `llama-cpp` feature implements the first adapter in `src/local_runtime/llama_cpp.rs`.
- M01-002 was source-reviewed as PASS. The supplied `Qwen3-8B-Q4_K_M.gguf` smoke passed end-to-end through `LocalInferenceRuntime` → `LlamaCppRuntime` → llama.cpp on CPU; Vulkan/GPU optimization remains separate from M01-002.
- Memory is explicit Orc data, separate from model weights.
- Lead/Planner reasoning will move into Controller after the Controller foundation is proven; durable useful Plan/decision data is preserved.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Implement M02-001: build a bounded, model-independent Controller state packet
from `ProjectOperations` / selected `OrcApp` read APIs and return a typed,
read-only recommendation through `LocalInferenceRuntime`. Use a fake runtime
for deterministic tests; do not grant mutation permission or connect the
Controller to lifecycle, database mutation, planning, Lead, memory or tools.

See `M00-REPOSITORY-MAP.md` for the repository-grounded migration map.
