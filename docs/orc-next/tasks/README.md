# Orc Next Task Index

Only current/near-term milestones are decomposed here.

| Task | Status | Milestone | Objective |
|---|---|---|---|
| [M00-001](M00-001.md) | Done | M00 | Map current Orc into kernel, policy and interface surfaces |
| [M01-001](M01-001.md) | Done | M01 | Introduce the model-independent local runtime boundary |
| [M01-002](M01-002.md) | Done | M01 | Integrate llama.cpp behind the local runtime boundary |
| [M02-001](M02-001.md) | Done | M02 | Build the read-only Controller state/recommendation path |
| [M02-002](M02-002.md) | Done | M02 | Evaluate read-only Controller decision quality |
| [M02-003](M02-003.md) | Done | M02 | Enforce reliable structured Controller output |
| [M03-001](M03-001.md) | Done | M03 | Define typed Controller action intents and legality boundary |
| [M03-002](M03-002.md) | Done | M03 | Execute explicitly authorized Controller intents |
| [M03-003](M03-003.md) | Done | M03 | Connect Controller recommendations to supervised typed actions |

## Task format

Each task records:
- ID and status;
- milestone;
- objective and why;
- scope;
- non-goals;
- dependencies;
- acceptance criteria;
- required tests/evidence;
- implementation notes/decisions;
- result.

A task should be independently reviewable. Architectural assumptions discovered during execution must be reflected in the canonical architecture/decision docs before dependent work proceeds.
