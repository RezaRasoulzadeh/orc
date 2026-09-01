# pocket-ledger-v1 autonomy fixture

This versioned fixture recreates Orc's external five-task benchmark without any
of the lost `/tmp` repositories or SQLite registries. It is the canonical
baseline for future comparisons, not a claim of byte-for-byte recovery of the
lost experiment. See [provenance.md](provenance.md).

## Controlled and external state

Source-controlled material includes the seed repository, five ordered task
contracts, validation commands, provider/model/action/capability declarations,
economy costs, deterministic Git identity and timestamps, setup scripts, and
manifest schema.

Provider credentials, account authentication, live quota, and the actual
profile directory remain external. No secret, token, live registry database,
or generated project database belongs in this fixture. `TRIAL_PROFILE_PATH`
must point to an authenticated Codex profile; setup verifies it with
`codex login status` but sends no model request.

## One-command preparation

Build Orc first, then choose durable repository and evidence locations. The
target and run directory must not already exist.

```sh
cargo build
TRIAL_PROFILE_PATH=/path/to/authenticated/CODEX_HOME \
  fixtures/autonomy/pocket-ledger-v1/scripts/prepare-trial.sh \
  /durable/path/pocket-ledger-v1-run-1 \
  /durable/path/orc-trial-results \
  task5-run-1 \
  "$PWD/target/debug/orc"
```

Preparation performs no AI invocation. It deterministically generates and
commits the seed, adopts it with the current Orc build, commits the adopted
configuration, onboards and attaches the two declared agents, configures
economy tiers, atomically applies the five contracts, verifies default-tier
selection for T-0001, checks for Orc self-hosting instructions, and writes the
initial evidence manifest.

The seed commit must be
`149d45a00b6d25d7bebcbcfaed398c59231dd376`. The configured baseline commit is
recorded rather than hard-coded because adopted-project documents are an input
from the tested Orc version.

## Baseline verification

```sh
fixtures/autonomy/pocket-ledger-v1/scripts/verify-baseline.sh \
  /durable/path/pocket-ledger-v1-run-1
```

This runs the tracked, offline validation commands and requires a clean Git
worktree.

## Normal five-task procedure

Use the registry created under the run's evidence directory:

```sh
export ORC_GLOBAL_REGISTRY_PATH=/durable/path/orc-trial-results/task5-run-1/registry/agents.db
cd /durable/path/pocket-ledger-v1-run-1

orc dispatch T-0001
orc review --automated T-0001
# If REVISE: orc revise T-0001, then review again.
# If PASS:   orc task accept T-0001.

# Repeat the same explicit lifecycle for T-0002 through T-0005 in order.
```

Do not pass `--agent` or model overrides during benchmark execution. Repairs
remain Orc-owned lifecycle actions; revisions occur only after Review requests
them. Do not manually edit task worktrees or databases.

Task 5 intentionally states that Suspended records do not count as active, but
the seed contains no Suspended state and no deterministic test that directly
asserts that future semantic rule. This preserves the semantic Review case.

## Focused Task 5 rerun #1

To start the next gate from a fresh prepared repository after Tasks 1–4 have
been completed and accepted normally:

```sh
export ORC_GLOBAL_REGISTRY_PATH=/durable/path/orc-trial-results/task5-run-1/registry/agents.db
cd /durable/path/pocket-ledger-v1-run-1
orc dispatch T-0005
orc review --automated T-0005
```

If Review returns REVISE, run `orc revise T-0005` followed by another automated
review. Accept only a semantically correct PASS. The requested two expensive
focused reruns are deliberately not automated by this fixture.

## Evidence capture

After any lifecycle boundary or completed run:

```sh
fixtures/autonomy/pocket-ledger-v1/scripts/capture-results.sh \
  /durable/path/pocket-ledger-v1-run-1 \
  /durable/path/orc-trial-results/task5-run-1 \
  /path/to/orc
```

The result directory retains the initial manifest, seed/configured commits,
task plan and ID map, agent/economy snapshots, scheduler explanation, per-task
status, provider usage and packet metadata, Review/revision/repair evidence,
and final Git state. Raw prompts and credentials are not captured.
