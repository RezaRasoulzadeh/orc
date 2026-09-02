# Orc Next Status

**Architecture:** Orc Next / Controller + deterministic kernel

**Current milestone:** M00 — Architecture and repository mapping

**Current task:** M00-001 — Map current Orc into kernel, policy and interface surfaces

**Last completed:** Project-control structure established

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one.
- Initial candidate: Qwen3 8B through llama.cpp/GGUF.
- Model runtime must remain replaceable.
- Memory is explicit Orc data, separate from model weights.
- Lead/Planner reasoning will move into Controller after repository mapping.
- Preserve Dispatch/Review/Revise/Accept execution primitives.
- Rust/native runtime; avoid Python.

## Immediate next action

Execute M00-001 as a repository-reading/design task. Produce a concrete map of current modules/APIs and classify each important behavior as deterministic kernel, engineering policy/judgment, interface adapter, or candidate for removal/migration.

Do not implement the Controller or delete Lead/Planner during M00-001.
