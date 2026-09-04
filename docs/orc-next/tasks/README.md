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
| [M04-001](M04-001.md) | Done | M04 | Expose bounded recovery facts and legal recovery operations |
| [M04-002](M04-002.md) | Done | M04 | Add read-only Controller recovery choice |
| [M04-003](M04-003.md) | Done | M04 | Execute explicitly authorized recovery recommendations |
| [M04-004](M04-004.md) | Done | M04 | Route validation-repair exhaustion into supervised Controller recovery |
| [M04-005](M04-005.md) | Done | M04 | Route semantic revision non-convergence into supervised Controller recovery |
| [M05-001](M05-001.md) | Done | M05 | Add read-only Controller planning capability |
| [M05-002](M05-002.md) | Done | M05 | Make persisted Plan provenance Controller-compatible |
| [M05-003](M05-003.md) | Done | M05 | Persist Controller Plan proposals through explicit authorization |
| [M05-004](M05-004.md) | Done | M05 | Add read-only Controller Plan review judgment |
| [M05-005](M05-005.md) | Done | M05 | Persist Controller Plan review decisions through explicit authorization |
| [M05-006](M05-006.md) | Done | M05 | Add read-only Controller Plan revision generation |
| [M05-007](M05-007.md) | Done | M05 | Persist Controller Plan revisions through explicit authorization |
| [M05-008](M05-008.md) | Done | M05 | Route supervised Plan workflow through Controller capabilities |
| [M05-009](M05-009.md) | Done | M05 | Replace Lead intake judgment with supervised Controller routing |
| [M06-001](M06-001.md) | Done | M06 | Establish typed persistent memory records and deterministic storage |
| [M06-002](M06-002.md) | Planned | M06 | Add deterministic bounded memory retrieval to Controller context |

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
