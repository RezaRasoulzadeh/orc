# Orc Next Decisions

Durable decisions belong here so future chats/agents do not have to reconstruct why the architecture changed.

## D-001 — Intelligent Controller over deterministic kernel

**Status:** Accepted

The local Controller owns engineering judgment. The deterministic Rust kernel owns facts, invariants and safe execution.

Reason: beta use showed that encoding every recovery/economy/continuation judgment as deterministic policy creates composition failures and operator dead ends.

## D-002 — Preserve existing Orc; evolve rather than rewrite

**Status:** Accepted

Existing task, run, review, validation, worktree, persistence, registry and application infrastructure is valuable. Repository mapping precedes architectural removal.

## D-003 — Local Controller from phase one

**Status:** Accepted

Do not use an external provider as the Controller bootstrap. External models remain coding/review workers. The Controller itself starts local.

## D-004 — Initial model/runtime direction: Qwen3 8B + llama.cpp/GGUF

**Status:** Accepted, subject to benchmark during M01

Use Qwen3 8B as the initial Controller candidate and llama.cpp/GGUF as the native inference direction. Keep model/tokenizer/prompt/runtime details behind a replaceable boundary so another model can replace Qwen without Controller/kernel redesign.

## D-005 — Memory belongs to Orc, not model weights

**Status:** Accepted

User/project facts and experience are stored explicitly by Orc. Relevant memory is retrieved into Controller context. Fine-tuning teaches reasoning behavior; it is not the primary storage mechanism for personal/project facts.

## D-006 — Controlled learning, not continuous weight mutation

**Status:** Accepted

Collect verified Controller decisions, operator corrections and outcomes. Fine-tune periodically against a curated dataset and evaluation suite. Never blindly train after every interaction.

## D-007 — Rust/native runtime; avoid Python

**Status:** Accepted

Orc runtime remains Rust/native. C/C++ is acceptable for inference/performance components. Do not introduce Python runtime dependencies. Training/export is a separate future concern and native options should be investigated first.

## D-008 — Lead and Planner become Controller capabilities

**Status:** Accepted direction, migration pending

Separate Lead/Planner intelligent roles are not the target. Preserve useful Plan/approval artifacts where justified. Do not delete current machinery until M00 maps dependencies and migration seams.

## D-009 — Repository docs are project-control authority

**Status:** Accepted

`docs/orc-next` is the durable source of truth for target architecture, roadmap, decisions, current status and milestone tasks. New chats should start with README + STATUS and open deeper documents as needed.

## D-010 — Detail tasks only near execution

**Status:** Accepted

Keep the long roadmap milestone-level. Decompose the current/next milestone into independently reviewable tasks only when repository evidence supports the scope. This avoids stale speculative task plans.
