# Provenance

`pocket-ledger-v1` is the canonical reproducible baseline for future external
autonomy trials. It is not claimed to be byte-for-byte identical to the lost
ephemeral repository.

Exact material recovered from preserved evidence:

- the five task titles and complete objective strings used by the recorded
  trial;
- task ordering, context files, expected changes, and the absence of task
  dependencies;
- validation commands;
- the dependency-free Rust module/CLI shape;
- the `gpt-5.6-luna` default-tier and `gpt-5.6-terra` escalation-tier setup;
- the generic adopted-project engineering contract requirement; and
- observed trial results and provider usage in the narrative report.

Reconstructed material:

- the exact bytes of the original `pocket-ledger` source tree, which were lost
  with the temporary repository;
- supplemental decomposition of each recovered objective into ordered
  acceptance criteria, required tests, unchanged behavior, and validation
  arrays required by current `PlanResponse`;
- deterministic Git author, timestamps, branch name, and commit messages; and
- fixture setup/result scripts, which did not exist in the original run.

Canonical v1 assumption:

- `Suspended` is a distinct state and must not increment the `active` summary
  count. This is explicit in Task 5's semantic acceptance criteria. The seed's
  deterministic tests intentionally do not encode this future-state rule, so
  semantic Review remains responsible for detecting a faulty interpretation.
