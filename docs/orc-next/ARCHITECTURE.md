# Orc Next Architecture

## Definition

Orc is a persistent, memory-enabled local engineering agent that reasons about projects and delegates software work to economical external workers through a deterministic Rust execution kernel.

## Shape

```text
Operator
   ↓
Local Orc Controller
   │ structured intents
   ↓
Deterministic Rust Kernel
   ├── persistence / transactions
   ├── tasks / plans / lifecycle invariants
   ├── Git / worktrees
   ├── deterministic validation
   ├── permissions / approvals
   ├── agent registry / quota facts
   └── evidence / history
   │
   ├── Coding workers
   └── Review workers
```

## Controller

The Controller owns judgment: planning, next-action selection, failure interpretation, recovery recommendations, worker/model selection, economy reasoning, operator interaction, and memory retrieval/consolidation.

The Controller cannot mutate canonical state directly. It emits schema-constrained intents against a small high-level tool surface.

Initial Controller direction: local Qwen3 8B through a native llama.cpp/GGUF runtime. Model-specific details must remain behind a replaceable runtime boundary so changing model size/family does not redesign Orc. `src/local_runtime.rs` defines that boundary as `LocalInferenceRuntime`, with bounded model-independent request/response/error types and separate runtime configuration. The optional `llama-cpp` Cargo feature exposes `LlamaCppRuntime`, which consumes only `LocalRuntimeConfig` and implements the trait; llama.cpp handles, GGUF loading, tokenization and sampling remain private to `src/local_runtime/llama_cpp.rs`. No Controller state or kernel mutation crosses it.

## Kernel

The kernel owns facts and invariants. It must remain authoritative for task/plan/run records, dependencies, worktrees, validation evidence, review/revision lineage, permissions, approval gates, legal transitions, transactional mutation and cancellation/acceptance.

Models cannot fabricate PASS evidence or bypass kernel constraints.

## Normal task lifecycle

```text
Ready
  ↓ dispatch
Active
  ↓ implementation + deterministic validation
Review
  ├── PASS → AcceptanceReady → explicit accept → Done
  └── REVISE → RevisionRequired → revise → Active
```

Abnormal engineering judgment should not grow into an exhaustive deterministic policy tree. The kernel records the facts and exposes safe recovery operations; the Controller selects among them.

## Planning and Lead

Planning remains a useful capability and Plan may remain a persisted artifact. Separate Planner and Lead intelligent roles are not the target architecture. Their judgment moves into the Controller. Existing machinery is migrated only after repository mapping proves what is safe to simplify.

## Validation

Validation execution and evidence remain deterministic. The Controller decides what to do with a failure. Avoid fixed blind repair loops; repeated equivalent failure signatures should discourage another identical model call.

## Economy

Use the cheapest competent external worker. Escalate only when evidence justifies it. The Controller reasons about expected value; the kernel enforces hard availability, permission, quota and operator-policy constraints.

## Memory

Memory is separate from model weights.

- Working memory: bounded current context.
- User memory: durable cross-project preferences.
- Project memory: architecture, decisions, conventions, components and history.
- Episodic memory: significant events.
- Experience memory: reusable lessons across projects.

Precedence:

```text
current operator instruction
→ current project facts/decisions
→ current task/plan contract
→ durable user preferences
→ historical experience
→ base model tendencies
```

Memory must be inspectable, correctable and removable. Structured facts are primary; semantic/vector retrieval is supplementary.

## Learning

Do not continually mutate weights from raw interactions. Collect verified state → decision → correction/approval → outcome examples. Fine-tuning is periodic and controlled. Training teaches Orc how to reason; memory tells it precise user/project facts.

## Technology constraints

- Rust is the primary Orc implementation language.
- Native C/C++ is acceptable for inference/runtime components.
- Avoid Python in the Orc runtime.
- Local Controller inference should have a replaceable model/runtime boundary.
- CLI, TUI and GUI are adapters over the same canonical application/kernel APIs.

## Anti-goals

Do not rewrite Orc from scratch, let models directly control SQLite, duplicate business logic in interfaces, retain Lead+Planner+Controller judgment indefinitely, build another giant recovery state machine, dump entire repositories into prompts, or use expensive workers to compensate for orchestration defects.
