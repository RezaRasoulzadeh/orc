# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M07 — Supervised autonomy

**Current task:** M07-011 — Compose one selected Controller memory maintenance step

**Last completed:** M07-010 — Add bounded Controller memory maintenance target selection

**Blocked by:** Nothing

## Current decisions

- Local Controller remains Qwen3 8B through llama.cpp/GGUF behind model-independent `LocalInferenceRuntime`.
- Controller owns judgment; deterministic kernel/application code owns canonical facts, legality, authorization, persistence, workflow transitions, validation, and mutation.
- M06 is complete: typed durable memory, bounded deterministic retrieval, capability-local memory context, supervised mutation, explicit-candidate capture judgment, and explicit-target maintenance judgment.
- M07-001..005 provide finite grant-aware routine task continuation through the existing workflow engine; Acceptance/user/external gates remain authoritative.
- M07-006/M07-007 provide separate finite Project/Episodic capture Create permission and one-step explicit capture composition. Automatic capture candidate derivation remains unresolved because the capture request contains a full `MemoryDraft`; deterministic code must not invent what should be remembered.
- M07-008/M07-009 provide separate finite Project/Episodic Correct/Supersede/Remove maintenance permission and one-step explicit maintenance composition.
- M07-010 is complete. `OrcApp::select_controller_memory_target(...)` performs bounded read-only selection from canonical active exact-current-project Project/Episodic candidates and returns only `NoTarget` or one exact supplied candidate.
- M07-010 filtering, ordering, bounds, omission metadata, and output validation are deterministic. User/Experience/global/historical/cross-project records remain excluded. Real-Qwen evaluation passed strict 7/7 and semantic 7/7.
- The next smallest M07 seam is composition rather than event automation: one M07-010 selection followed by at most one existing M07-009 maintenance call.
- M07-011 must reuse the exact same caller-supplied `current_facts` for both selection and M06-011 maintenance judgment. Application code must not derive or reinterpret maintenance evidence.
- A selected target must be freshly re-resolved by existing M06-011/M07-009 logic; the M07-010 candidate record must not be cached as mutation authority.
- M07-011 may perform at most two Controller inference calls total, one proposal, one authorization mint, and one execution attempt, with no retry, alternate target, omitted-candidate fallback, or batch loop.
- `NoTarget`, selection failures, Keep, and all pre-mint maintenance failures consume zero maintenance budget; successful authorization consumes one; post-mint failure is not refunded.
- Automatic current-fact derivation, workflow/task/Plan/review/validation/recovery hooks, background maintenance, batch maintenance, User/Experience/global mutation, semantic/vector retrieval, and embeddings remain out of scope.
- Continuation, capture, and maintenance grants remain distinct in-process capability budgets. No provider token hard cap.
- Rust/native runtime; no Python.

## Immediate next action

Implement `tasks/M07-011.md`: compose one bounded target-selection judgment with at most one existing explicit-target maintenance operation using the same current facts and existing maintenance grant.

See `M00-REPOSITORY-MAP.md` for repository-grounded fact-versus-judgment classification and migration map.
