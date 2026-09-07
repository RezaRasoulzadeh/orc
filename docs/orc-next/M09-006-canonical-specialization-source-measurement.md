# M09-006 — Canonical specialization source measurement

**Measurement status:** Complete; training readiness is not assessed
**As of:** 2026-09-07
**Repository revision inspected:** `52a37429c8438e626106fd25fa23363df7496109` (local `HEAD` used for this measurement; not the latest remote `HEAD`)

## Scope and repository state

The requested `docs/orc-next/tasks/M09-006.md` task file was absent from the
checked-out local `main` at `52a3742`. `origin/main` was five commits ahead at
`0b386c9630eea081e85b4db5dd70117c6b44277a` and already contained the task file
through commit `647eeae079a89a887fbf787921d68385288a098a`. This report executes
the M09-006 measurement/provisioning scope provided by the operator request and
records that local-branch mismatch; it does not create or infer a missing
canonical task document.

This was a repository-grounded measurement only. No model or GGUF was
downloaded, no trainer or dependency was installed, no dataset was exported or
transformed, no training or adapter creation occurred, and no canonical
experience record was created, changed, retired, or deleted.

## Registry authority and provisioning

Orc's current global-registry authority is code-defined in
`src/storage/db.rs`:

1. `ORC_GLOBAL_REGISTRY_PATH`, when set;
2. `$XDG_DATA_HOME/orc/agents.db`, when `XDG_DATA_HOME` is set;
3. `$HOME/.local/share/orc/agents.db`;
4. `.orc-global/agents.db` only when `HOME` is unavailable.

At measurement time `ORC_GLOBAL_REGISTRY_PATH` and `XDG_DATA_HOME` were
unset, and `HOME=/home/reza`. The resolved global registry was therefore:

`/home/reza/.local/share/orc/agents.db`

The project database independently persisted the same absolute path in
`.orc/orc.db` under `meta.agent_registry_path`. No companion registry was
selected. This agrees with the existing `Database::default_global_registry_path`
and `Database::open_global`/`init_global` authority rules; the earlier M09-005
path finding was re-derived rather than assumed.

Before measurement, the resolved registry lacked the canonical
`controller_experience_examples` table. It was provisioned using the existing
operator-supported `orc init` path (`Database::init_global`); no SQL schema,
record, copy, seed, or fabricated example was supplied by this task. A
read-only check after provisioning found the table with the M08-001 columns and
zero rows. The supported command reported `Initialized Orc DB in
.orc/orc.db (project id=1)`. A missing table was therefore treated as
provisioning state, not as an empty dataset.

The repository has no dedicated CLI command for the M08-011 inventory or
M09-001 snapshot. The required existing application APIs are present and were
used through a temporary, non-production read harness: `OrcApp::open_global`,
`OrcApp::controller_experience_inventory`, and
`OrcApp::controller_experience_snapshot`. The harness was removed after the
measurement. The APIs themselves are the canonical read boundaries; no
production code was added for operator convenience.

## Canonical inventory and snapshot evidence

The complete M08-011 inventory API returned:

| Field | Measured value |
| --- | ---: |
| Inventory schema version | `1` |
| Total examples | `0` |
| Active examples | `0` |
| Retired examples | `0` |
| Per-capability summaries | empty |

The M09-001 Active snapshot API returned:

| Field | Measured value |
| --- | ---: |
| Snapshot schema version | `1` |
| Exact Active count | `0` |
| Example IDs | none |
| Deterministic order | empty sequence; no IDs to order |
| Examples | empty |

This is a measured zero after normal schema provisioning, not an unavailable
or unknown count. Because the canonical table contains no rows, there are no
canonical lifecycle, verification-basis, outcome, quality, provenance,
correction, capability, or payload values to summarize. The inventory API
correctly returns no synthetic capability rows for absent capabilities; no
quality or readiness distribution is inferred.

For the typed categorical fields exposed by M08-011, the observed distributions
are exactly zero rows in every category: lifecycle `Active=0`, `Retired=0`;
outcome `Accepted=0`, `Corrected=0`, `Rejected=0`; and verification basis
`OperatorAttestation=0`, `ExplicitCorrection=0`, `ExternalEvaluation=0`.
These are empty-dataset counts, not labels or suitability judgments. No quality
score distribution is available because the inventory is metadata-only and the
M09-001 snapshot contains no examples.

The typed boundaries remain authoritative: M08-011 reads the complete global
registry metadata set and returns deterministic counts; M09-001 reads the
complete canonical Active set, validates each typed example, and orders rows by
ascending canonical ID. Neither boundary performs export, trainer formatting,
sampling, splitting, balancing, ranking, weighting, or readiness judgment.

## Nine-capability coverage

M08-011 defines the current nine capability identifiers. The measured complete
dataset has zero examples in every one because its total row count is zero.
The API returned an empty capability-summary list, so these are coverage
results against the M08-defined capability universe, not fabricated inventory
records:

| Canonical capability | Total records | Active records | Coverage |
| --- | ---: | ---: | --- |
| `controller.task_recommendation` | 0 | 0 | no observed examples |
| `controller.recovery_recommendation` | 0 | 0 | no observed examples |
| `controller.plan_generation` | 0 | 0 | no observed examples |
| `controller.workflow_intake` | 0 | 0 | no observed examples |
| `controller.plan_review` | 0 | 0 | no observed examples |
| `controller.plan_revision` | 0 | 0 | no observed examples |
| `controller.memory_capture` | 0 | 0 | no observed examples |
| `controller.memory_maintenance` | 0 | 0 | no observed examples |
| `controller.memory_selection` | 0 | 0 | no observed examples |

Observed capability coverage is therefore `0/9`; this does not establish a
required balance, target, minimum dataset size, or training suitability.

## Other readiness evidence

The following read-only checks were repeated for the current environment:

| Prerequisite | State | Evidence |
| --- | --- | --- |
| Canonical registry/table | Provisioned; measured empty | Resolved registry contains `controller_experience_examples`; canonical APIs returned total `0`, Active `0`, Retired `0`. |
| Nine-capability source coverage | Measured zero | Inventory returned no capability rows; M08-011 defines the nine identifiers listed above. |
| Qwen3 checkpoint | Unavailable locally | No local Qwen/Hugging Face model directory was found under the inspected repository, cache, or temporary paths. |
| GGUF baseline artifact | Unavailable locally | No `*.gguf` file was found and `ORC_QWEN3_GGUF` was unset. |
| Baseline score | Unmeasured / absent | M09-003 records that no real-Qwen score was executed when `ORC_QWEN3_GGUF` was unavailable; this task did not run inference. |
| Training GPU | Unavailable | `nvidia-smi` could not communicate with the NVIDIA driver. |
| Host CPU/RAM | Measured, insufficient for the documented GPU example | x86_64 Intel Core i5-14600K; 14 physical / 20 logical CPUs; 14 GiB RAM, 4.5 GiB available at this inspection; 7.5 GiB swap. This does not prove every CPU configuration impossible. |
| Trainer/toolchain | Not prepared | No trainer was installed and no training/export environment was selected. |

The Qwen 22 GB figure remains the documented ms-swift LoRA example context from
M09-005, not a universal hardware requirement. It is not converted here into a
dataset-size, quality, or trainer-suitability claim.

## Preserved boundaries and blockers

- Backend selection remains deferred. The conditional ms-swift path from
  M09-005 remains only a candidate and was not selected or run.
- The canonical training-input authority, if later approved, remains the
  M09-001 typed Active snapshot. No trainer-specific representation is created
  here.
- M09-002 remains the fixed nine-capability semantic evaluation authority;
  M09-003 remains the baseline/report boundary; M09-004 remains the only
  candidate comparison/promotion gate.
- D-006 remains controlled, periodic learning from curated verified records;
  D-007 keeps the Orc production runtime Rust/native and keeps any future
  training/export concern separate. No runtime, model default, data format,
  architecture, or promotion decision changed.
- Native llama.cpp training remains a separately qualified possibility only;
  its upstream WIP/limited training evidence does not establish a Qwen3-8B
  training path. No GGUF-to-trainer or adapter compatibility is assumed.
- Canonical data is not ready for specialization execution: the measured
  complete source contains zero records, and no quality, balance, threshold,
  split, weighting, sampling, ranking, or readiness policy may be inferred.
- M09-005's pre-provisioning statement that the experience table was absent is
  superseded by this task's supported schema provisioning and measurement; its
  backend-deferred and runtime-boundary conclusions remain unchanged.
- The absence of a dedicated operator CLI read path is a usability seam, not a
  data-authority gap: the required canonical application APIs exposed the
  evidence without a production-code change. A future read-only CLI command
  would be an independently reviewable tooling improvement, not part of
  M09-006.

## Completion evidence

Repository sources inspected included the global registry authority and schema
provisioning in `src/storage/db.rs`, the supported `init` path in `src/main.rs`,
the `OrcApp` inventory/snapshot APIs in `src/app.rs`, M08-001 and M08-011,
M09-001 through M09-005, D-006/D-007, and the existing tests covering inventory
and snapshot determinism/validation.

Only this report was added. No production source, dependency, canonical task,
status, roadmap, model, dataset record, or runtime default was changed by this
task. The registry schema provisioning was performed through Orc's normal
operator path and is outside the repository diff; no canonical records were
written. `git diff --check` passed. No code validation was run because the
repository change is documentation-only.
