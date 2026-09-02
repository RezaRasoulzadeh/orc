# Orc Next

Canonical control center for Orc's next architecture.

## Product direction

Orc is becoming a persistent, memory-enabled local engineering agent backed by a deterministic Rust execution kernel. A local Controller model owns judgment; the kernel owns facts, permissions, validation, persistence, Git/worktrees, evidence, and legal transitions. External coding/review models remain workers.

## Core rules

- Hardcode invariants, not engineering judgment.
- The Controller proposes typed actions; the kernel validates and executes them.
- Memory belongs to Orc, not model weights.
- Planning and Lead-like reasoning move into the Controller over time; useful Plan data may remain persisted.
- Preserve Dispatch -> Review -> Revise -> Accept as deterministic execution primitives.
- Prefer cheap worker models and avoid model calls that add no information.
- Runtime remains Rust/native; avoid Python.
- CLI, TUI and GUI must share the same canonical core APIs.
- Do not rewrite Orc from scratch.

## Documents

- [ARCHITECTURE.md](ARCHITECTURE.md) — target architecture and boundaries.
- [ROADMAP.md](ROADMAP.md) — milestones.
- [DECISIONS.md](DECISIONS.md) — durable architectural decisions and reasons.
- [STATUS.md](STATUS.md) — short current-state handoff for humans and agents.
- [tasks/README.md](tasks/README.md) — task index and task format.

## Control hierarchy

```text
ARCHITECTURE
    ↓
ROADMAP
    ↓
MILESTONE
    ↓
TASK
    ↓
IMPLEMENTATION
```

If implementation changes an architectural assumption, update these documents before building further on the new assumption.
