# External Autonomy Trial — 2026-09-01

## Decision

**NOT READY FOR SELF-HOSTING**

The provider-context follow-up battery also does not meet the self-hosting
gate. Context cost is now attributable and aggregate use fell 15.4%, but only
four of five clean-rerun tasks were semantically acceptable. Task 5 passed
deterministic validation and automated Review while incorrectly counting
`Suspended` records as `active`; it was deliberately not accepted or manually
repaired. See **Provider-context follow-up** below.

Orc completed all five maintenance implementations on the cheapest configured
tier without operator code edits or model overrides. Four initial task
lifecycles were clean. One reached `Done` with a contradictory review ledger:
the review verdict was `PASS`, but the same response persisted a new blocker.
That initial task is therefore classified as a failure. The invariant was fixed
directly in Orc and the affected task then completed from a clean repository.

The final code outcomes are encouraging, but this run does not yet establish
reliable and economical self-hosting. The initial run found three production
defects, and 17 invocations consumed 777,051 tokens for five small tasks despite
bounded Orc packets. A second clean five-task run is needed after the fixes and
after provider-context cost is made observable and materially smaller.

## Repositories and controls

- Fixture: dependency-free Rust `pocket-ledger`, baseline commit `0c797d6`.
- Primary trial: `/tmp/orc-autonomy-trial-rerun-20260831`.
- Preserved pre-dispatch adoption failure:
  `/tmp/orc-autonomy-trial-20260831`.
- Clean rerun of the failed lifecycle:
  `/tmp/orc-autonomy-trial-task1-rerun-20260901`.
- Real Git branches, worktrees, commits, merge commits, source, and tests were
  used. The fixture was not vendored into Orc.
- Validation was `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, and `cargo test`.
- The operator invoked only legitimate Orc lifecycle commands. No external
  task code, blocker, database, or lifecycle state was manually repaired.

The fixture has five modules (`model`, `parser`, `normalize`, `catalog`, and
`summary`), a CLI consumer, and integration tests. It is small and deterministic
but has enough module boundaries for realistic maintenance work.

## Task set and results

Every task contract stated its objective, acceptance behavior, required tests,
expected files, non-goals, relevant context, and the three deterministic
validation commands.

| Task | Category and contract | Initial result | Invocations | Validation repairs | Semantic revisions | Tier / escalations | Input / cached / output / total tokens |
| --- | --- | --- | ---: | ---: | ---: | --- | ---: |
| T-0001 | Bug fix: collapse Unicode label whitespace; direct and catalog regression tests; no public API redesign | **FAILURE**: code and validation were correct, but `PASS` persisted a new blocker; clean post-fix rerun **SUCCESS** | 6 | 2 | 1 | default / 0 | 242,687 / 139,008 / 4,387 / 247,074 |
| T-0002 | Feature: bounded normalized label-prefix query, insertion order, inactive records, zero/empty limits | **SUCCESS** | 2 | 0 | 0 | default / 0 | 83,499 / 58,368 / 1,376 / 84,875 |
| T-0003 | Refactor: extract shared required-field validation while preserving parser/model behavior | **SUCCESS** | 3 | 1 | 0 | default / 0 | 115,272 / 63,488 / 1,636 / 116,908 |
| T-0004 | Test improvement: parser matrix for IDs, fields, whitespace, and case-insensitive states | **SUCCESS** | 3 | 1 | 0 | default / 0 | 99,454 / 48,384 / 1,499 / 100,953 |
| T-0005 | Multi-file change: replace boolean state with `RecordState`, add `Suspended`, update parser/catalog/summary/CLI/tests | **SUCCESS** | 3 | 1 | 0 | default / 0 | 223,667 / 156,672 / 3,574 / 227,241 |

Task 1's clean rerun used 3 default-tier invocations, 1 formatting repair, no
semantic revision or escalation, and 111,273 input, 60,416 cached input, 1,786
output, and 113,059 total tokens. Its first review was a consistent `PASS` with
no blockers, it was explicitly accepted, and the repository passed all checks.

Strict initial success is 4/5. After fixing the exposed lifecycle defect and
rerunning only the affected scenario from clean state, all five task contracts
have a clean successful lifecycle. This is an observation from a small sample,
not a general success-rate claim.

## Model and economy behavior

The project configured relative model costs, not provider-specific experiment
branching:

- `gpt-5.6-luna = 1`: `default`
- `gpt-5.6-terra = 2`: `escalation`
- `gpt-5.6-sol = 4`: `exceptional`
- unlisted models: `unknown`

Before every primary task, `orc schedule ... --explain` selected `trial-cheap`
from the cheapest eligible `default` tier. Dispatch/revision work used low
effort. Review actions used the review template's high effort while remaining
on the same cheap model and tier. The cheap agent had fresh quota observations
(68% initially, 65% by Task 5); the stronger candidate was eligible but either
fresh or refresh-required and was never selected. There were no operator model
overrides, policy escalations, or unexplained tier changes.

Persisted ProjectOperations economy totals for the primary trial were:

- 17 provider invocations: 11 code and 6 review.
- All 17 on `default`; escalation count 0.
- 764,579 input, 465,920 cached input, 12,472 output, 777,051 total.
- Cached-input ratio 60.94%; uncached input 298,659.
- Accepted-work metric: 155,410.2 tokens per accepted database task.

The configured costs are relative tier weights, not currency prices, so a
monetary accepted-work cost cannot be calculated honestly. The token result is
not economical for tasks of this size even though tier selection was correct.

## Lifecycle, convergence, and restart

The primary trial produced 12 persisted runs and five explicit merge commits.
There were five validation repairs, all bounded to current deterministic
failures: four rustfmt repairs and one unresolved-import repair. Only Task 1
required semantic revision: the reviewer rejected an unnecessary public
re-export, the revision worker received the persisted blocker without operator
translation, validation reran, and the next review passed.

An intentional restart boundary occurred after Task 1 dispatch and successful
validation, before semantic review. New Orc processes reported the task in
`Review`, retained passing validation evidence and its worktree fingerprint,
retained the ResolutionRecord and provider usage, and exposed the same facts
through task/economy read models. The subsequent review, revision, validation,
review, and acceptance completed normally. Blockers also survived the later
review-to-revision process. No escalation state existed to test in this sample.

## Provider packet observations

Actual provider session records were inspected, rather than estimating packet
size from token usage.

- Task 5 dispatch: 20,533 characters; metadata reported `dispatch`, 7 files,
  zero diff/diagnostics/blockers, and no truncations.
- Task 5 validation repair: 17,107 characters; one `cargo fmt --check` failure,
  2,368 diagnostic bytes, 7 selected files, and no truncations. It contained
  current failure diagnostics, not repository history.
- Task 5 semantic review: 15,996 characters; 9,775-byte current diff, 7 changed
  files, all three passing validation commands, zero blockers, no truncations.
- Task 1 revision: 16,215 characters; 4 relevant files, 1,911-byte current diff,
  exactly one actionable blocker, no operator feedback, and one deterministic
  plan-snapshot truncation of 2,241 bytes.

These observations validate role-specific bounded Orc packets. They do not
explain the much larger provider input counts (roughly 45,000–128,000 input
tokens per invocation). The Codex runtime adds system, tool, and environment
context outside the rendered Orc packet. Orc currently exposes packet metadata
inside the provider prompt but not through ProjectOperations or the CLI, so the
trial had to inspect provider session files. Cost attribution and packet
metadata observability remain systemic gaps.

## Failures and fixes

### 1. Adopted repositories received Orc's self-hosting contract

- Classification: context/packet defect.
- Reproduction: run `orc adopt` in a fresh external repository; the generated
  `.orc/engineering.md` contained Orc-specific architecture rules and was sent
  in every provider packet.
- Preserved evidence: `/tmp/orc-autonomy-trial-20260831`.
- Violated invariant: adopted projects require a generic external-project
  contract, not Orc's own repository constitution.
- Fix: added a generic adopted-project contract asset, made adoption use it,
  and added exact regression coverage. The primary repository was adopted from
  clean state after this fix.

### 2. Fresh CLI projects could not configure or inspect economy state

- Classification: scheduler/economy observability defect.
- Reproduction: create/adopt a CLI-only project and attempt to configure model
  costs or inspect ProjectOperations economy totals without direct DB access.
- Violated invariant: the authoritative persisted economy configuration and
  read model must be operable from the product surface used in the trial.
- Fix: added `orc economy configure` and `orc economy show`, tests, and CLI
  documentation. All trial economy evidence then came from this read model.

### 3. `PASS` could persist a new structured blocker

- Classification: lifecycle/state-machine and blocker convergence defect.
- Reproduction: a reviewer response with `verdict: PASS`, empty
  `blocking_findings`, and a structured blocker with `status: new`. Task 1's
  second review persisted `BLK-a039c12fcdb19715` while accepting the task.
- Violated invariant: semantic PASS requires zero new, unresolved, or regressed
  blockers.
- Fix: after blocker canonicalization, Orc now recomputes blocking findings and
  changes a contradictory PASS to REVISE. The provider contract also states the
  conditional explicitly, and a focused regression test covers it.
- Rerun: Task 1 from baseline in a fresh adopted repository passed its first
  review with an empty blocker ledger and reached Done without escalation.

Two environment events were kept separate from semantic results: the sandbox
could not write Orc's default global registry path until an explicit disposable
registry was configured, and authenticated provider/quota operations required
the allowed network/profile boundary. Neither changed external task code or
lifecycle state.

## Final validation

The integrated primary fixture and the clean Task 1 rerun both passed format,
strict clippy, tests, and diff checks after acceptance. Orc production fixes are
validated with:

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
git diff --check
```

## Smallest remaining steps before self-hosting

1. Expose rendered packet metadata and per-invocation packet size through the
   ProjectOperations/CLI read model so token overhead can be attributed without
   provider-private session inspection.
2. Reduce or isolate the provider runtime context that turns 16–21 KB Orc
   packets into 45K–128K input-token invocations; define an evidence-based token
   budget for ordinary maintenance.
3. Run a second clean five-task external battery with the three fixes present
   from the start. Require consistent blocker ledgers, no manual state repair,
   and materially improved accepted-work token cost before a self-hosting trial.

## Provider-context follow-up

### Runtime layers and root cause

An inspected baseline Task 5 Dispatch session contained an 18,009-byte Codex
base-instruction object, a 4,845-byte developer message, a 5,324-byte world
state, about 4,529 bytes of environment/user wrapper, and a 21,738-byte Orc
user packet. The provider reported 128,046 input tokens because it charged
cumulative input across six internal model turns and five tool calls, not
because the single Orc packet contained 128K tokens. Most replay was cached.
Codex does not expose a per-source token split, so these byte measurements are
not presented as token counts.

The normalized context layers are now:

- Orc packet and deterministic top-level section sizes, including truncation
  metadata;
- fixed Orc action instructions and structured-output schema;
- known agent-profile instruction files, with ignored profile configuration
  measured but marked excluded;
- known repository instruction files when repository discovery is possible;
- execution environment, repository access/discovery flags, and new-session
  state; and
- provider bootstrap, system/developer instructions, tool schemas, and other
  integration context, explicitly marked unknown when the provider does not
  expose its breakdown.

The dominant remaining cost is provider runtime plus repeated internal turns.
Orc controls the packet, its fixed instructions/schema, profile/config launch
behavior, session flags, and action working directory. It does not control or
precisely observe Codex's base prompt, platform wrapper, built-in tool schemas,
or provider-side per-turn replay accounting.

### Reductions and observability

Every automated Codex invocation is a new ephemeral process and uses
`--ignore-user-config`; Orc never invokes `resume`. Lead, Planner, and semantic
Review use read-only non-repository directories. Mutation launcher processes
start isolated, while Codex's logical `--cd` and `--add-dir` are both anchored
to the task worktree. This last distinction was required after a diagnostic
run proved that using an isolated logical cwd could send a relative edit to the
main checkout. Mutation actions may therefore discover worktree instructions;
that is reported rather than hidden.

Dispatch, Revision, and repair prompts now tell the model to use the supplied
bounded files and deterministic evidence instead of rediscovering them.
Validation repair remains a fresh, bounded action. Review receives only its
contract, diff, validation evidence, and blockers and has no repository access
or implementation session history.

`ProviderInvocationContext` is persisted before transport beside each provider
invocation. `ProjectOperations` exposes it with provider usage after restart,
and economy summaries aggregate packet bytes, usage by action, attribution
coverage, and token/packet outliers. `orc task show` prints concise invocation
lines; `orc economy context [INVOCATION_ID]` prints size-only JSON and never the
raw prompt. Historical metadata is immutable when current profile files change.

### Authoritative five-task rerun

The final run used a freshly reconstructed and independently validated
dependency-free `pocket-ledger` fixture, committed Orc configuration, the same
five task contracts, one authenticated `gpt-5.6-luna` default-tier agent, and
only normal `dispatch`, automated `review`, `revise`, and `task accept`
commands. There were no operator agent/model overrides, escalations, manual
code edits, database edits, or lifecycle-state edits. All 19 invocations used
new sessions, had context metadata, and stayed on the default tier.

| Task | Result | Invocations | Repairs | Revisions | Input / cached / output / total |
| --- | --- | ---: | ---: | ---: | ---: |
| T-0001 | accepted after resolved scope blocker | 7 | 3 | 1 | 200,280 / 98,560 / 4,291 / 204,571 |
| T-0002 | accepted | 3 | 1 | 0 | 92,539 / 44,288 / 1,242 / 93,781 |
| T-0003 | accepted | 3 | 1 | 0 | 78,861 / 40,192 / 1,764 / 80,625 |
| T-0004 | accepted | 3 | 1 | 0 | 95,523 / 47,360 / 1,211 / 96,734 |
| T-0005 | **failed semantic gate; not accepted** | 3 | 1 | 0 | 177,168 / 124,160 / 4,380 / 181,548 |

Task 5's generated test encoded the same faulty interpretation as production:
with one Active, one Inactive, and one Suspended record it asserted `active ==
2`. Formatting, Clippy, tests, and automated Review all passed. The contract's
separate states, Active-only catalog rule, and retained `active` plus new
`suspended` counters require the counters to remain distinct. The result was
preserved at `acceptance_ready` rather than accepted or manually repaired.

### Before/after economics

The comparison includes every invocation, including the failed Task 5. The
after-run accepted-work metric is shown separately because only four tasks met
the semantic gate.

| Metric | Before | After | Change |
| --- | ---: | ---: | ---: |
| Input tokens | 764,579 | 644,371 | -120,208 (-15.7%) |
| Cached input | 465,920 | 354,560 | -111,360 (-23.9%) |
| Uncached input | 298,659 | 289,811 | -8,848 (-3.0%) |
| Output tokens | 12,472 | 12,888 | +416 (+3.3%) |
| Total tokens | 777,051 | 657,259 | -119,792 (-15.4%) |
| Tokens per battery task | 155,410 | 131,452 | -15.4% |
| Tokens per accepted task | 155,410 (5 accepted) | 118,928 (4 accepted) | not like-for-like |
| Average serialized packet | not persisted; samples about 16–21 KB | 9,690 bytes | exact change unavailable |
| Average rendered prompt | not persisted | 10,962 bytes | exact change unavailable |
| Attribution coverage | unavailable | 19/19 (100%) | +100 percentage points |

| Action | Before count / avg input | After count / avg input | Average-input change |
| --- | ---: | ---: | ---: |
| Dispatch | 5 / 70,252 | 5 / 57,647 | -17.9% |
| Validation repair | 5 / 53,755 | 7 / 30,937 | -42.4% |
| Revision | 1 / 53,219 | 1 / 51,401 | -3.4% |
| Review | 6 / 15,221 | 6 / 14,696 | -3.4% |

Average known additional context was 2,104 bytes and average serialized packet
size was 9,690 bytes, but all 644,371 provider-reported input tokens remain
unattributed at source-token granularity because Codex reports only aggregate
usage. The cached-input ratio fell from 60.94% to 55.02%. The modest 3.0%
uncached-input reduction, near-flat Review/Revision averages, and 124,738-token
Task 5 Dispatch show that provider bootstrap and multi-turn replay remain the
main cost. The large repair reduction demonstrates that Orc's bounded repair
context is materially cheaper, but does not make the overall economics small.

### Follow-up validation and recommendation

The accepted fixture main branch and the unaccepted Task 5 worktree each pass
`cargo fmt --check`, strict Clippy, `cargo test` (9 tests), and
`git diff --check`. Passing deterministic checks does not override Task 5's
semantic failure.

Recommendation: **NOT READY FOR SELF-HOSTING**. Context accounting and the
15.4% aggregate reduction are real improvements, but provider-side attribution
remains coarse and the clean battery exposed a semantic Review miss that would
have accepted incorrect behavior without independent scrutiny.

### Versioned rerun fixture

The temporary repositories and registry cited above were evidence locations,
not reproducible inputs, and no longer exist. The canonical source-controlled
baseline for subsequent comparisons is now
[`pocket-ledger-v1`](../fixtures/autonomy/pocket-ledger-v1/README.md). It
contains a deterministic reconstructed seed, the five recovered task titles
and objective strings, ordered current task contracts, validation and logical
agent/economy configuration, and scripts that create a fresh registry and
machine-readable evidence manifest without invoking a model.

The fixture does not claim byte-for-byte identity with the lost repository.
Its [provenance record](../fixtures/autonomy/pocket-ledger-v1/provenance.md)
marks recovered versus reconstructed material. In particular, Task 5 retains
the explicit requirement that Suspended records do not count as active while
the seed tests deliberately do not turn that semantic Review criterion into a
deterministic assertion.
