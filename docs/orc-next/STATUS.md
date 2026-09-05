# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M07 — Supervised autonomy

**Current task:** M07-010 — Add bounded Controller memory maintenance target selection

**Last completed:** M07-009 — Compose one supervised Controller memory maintenance step

**Blocked by:** Nothing

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M06 is complete: typed durable memory, bounded deterministic retrieval, capability-local memory context, supervised mutation, explicit-candidate capture judgment, and explicit-target maintenance judgment.
- M07-001..005 provide finite grant-aware routine task continuation through the existing workflow engine; Acceptance/user/external gates remain authoritative.
- M07-006/M07-007 provide separate finite Project/Episodic capture Create permission and one-step explicit capture composition. Automatic capture candidate derivation remains unresolved because the capture request contains a full `MemoryDraft`; deterministic code must not invent what should be remembered.
- M07-008/M07-009 provide separate finite Project/Episodic Correct/Supersede/Remove maintenance permission and one-step explicit maintenance composition.
- `OrcApp::maintain_controller_memory_once(...)` performs at most one M06-011 inference, one M06-009 proposal, one M07-008 authorization mint, and one M06-009 execution attempt. Keep/pre-mint failure consumes zero; successful mint consumes one; post-mint failure is not refunded.
- M07-009 public results are state-safe; M06-011 inference semantics remain unchanged.
- The remaining maintenance judgment before safe automatic-capable invocation is target selection. `MemoryService::list(...)` can deterministically enumerate current-project memory, but deterministic code must not decide which record warrants maintenance.
- M07-010 therefore adds read-only bounded Controller target selection from active exact-current-project Project/Episodic candidates plus explicit current facts. It returns no target or one exact candidate only.
- M07-010 does not invoke M06-011, inspect grants, mutate memory, select maintenance operations, derive current facts from workflow events, scan in background, or create a second orchestration loop.
- User/Experience/global/historical/cross-project records remain outside automatic-capable maintenance.
- Continuation, capture, and maintenance grants remain distinct in-process capability budgets. No provider token hard cap.
- Rust/native runtime; no Python.

## Immediate next action

Implement `tasks/M07-010.md`: bounded read-only Controller selection of zero or one exact active current-project Project/Episodic maintenance target from deterministic candidates and explicit current facts.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
