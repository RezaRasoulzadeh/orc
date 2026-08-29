# Orc Engineering Contract

This document is the mandatory engineering constitution for coding work performed on Orc.

It applies automatically to ordinary implementation, revision work, recovered or requeued
execution, and any other coding-agent execution path.

Task and revision instructions may specialize this contract but must not silently weaken,
override, or contradict it.

If satisfying a task requires violating this contract, stop and report:

ORC-ARCHITECTURE-DECISION: <required decision and reason>

Do not implement the conflicting architectural change without approval.


## 1. Engineering principles

- Use stable Rust.
- Prefer simple, explicit implementations.
- Prefer the standard library and existing project dependencies when appropriate.
- Add dependencies when they materially reduce risk, complexity, or duplicated infrastructure.
- Do not reimplement mature infrastructure merely to avoid a dependency.
- Evaluate established libraries before implementing terminal handling, parsers, protocols,
  concurrency primitives, serialization infrastructure, platform abstractions, or similar
  non-domain infrastructure from scratch.
- Do not introduce abstractions without a current requirement.
- Do not generalize for hypothetical future requirements.
- Do not modify unrelated behavior.
- Keep changes scoped to the task and the minimum supporting architecture required to implement it correctly.


## 2. Architecture

- `main.rs` is an entry point and routing layer; keep it thin.
- SQLite access and SQL belong in the storage layer.
- Persistent project state comes from SQLite unless an existing architecture explicitly defines otherwise.
- Do not place raw SQL outside storage.
- Provider-specific behavior belongs behind worker/backend abstractions.
- Shared application behavior remains provider-independent.
- Terminal/UI presentation must remain separate from orchestration and application logic.
- Shared behavior belongs in existing Orc application, orchestration, and shared core APIs rather
  than being duplicated by CLI,
  desktop, workers, reviewers, repair paths, or other frontends.
- Do not introduce hidden global mutable state.
- Respect ownership and thread-safety boundaries. Do not use unsafe `Send`/`Sync` implementations
  to bypass an architectural ownership problem.
- Public APIs may change only when required by the task or by a necessary approved architectural change.


## 3. Existing architecture before new architecture

Before adding a subsystem, abstraction, dependency, or architectural pattern:

1. Inspect the existing implementation and shared APIs.
2. Determine whether the required behavior already has an owner.
3. Reuse or extend that owner when appropriate.
4. Check whether an established dependency solves non-domain infrastructure better than custom code.
5. Introduce a new architectural boundary only when the existing design cannot correctly own the behavior.

Do not build parallel implementations of existing task, dispatch, review, validation, scheduling,
persistence, lifecycle, agent, Lead, Planner, or application behavior.


## 4. Correctness over superficial completion

Implement the requirement itself, not merely the smallest change likely to satisfy a test or reviewer.

Forbidden completion strategies include:

- tests that only have the requested name but do not exercise the required behavior;
- assertions against constants or duplicated test implementations instead of the production path;
- fake adapters that cannot produce the event/input/state being claimed as tested;
- changing output or status reporting without changing the underlying behavior;
- treating cancellation flags as cancellation when execution continues underneath;
- claiming streaming when events are buffered until completion;
- claiming portability when platform-specific production code is not structurally valid;
- satisfying a review finding literally while leaving the underlying requirement unresolved.

When a reviewer identifies a symptom, fix the underlying defect and check adjacent execution paths
for the same defect.


## 5. Requirement traceability

Before implementation, identify the task's observable requirements.

For every material requirement, determine:

- production path that implements it;
- state or persistence implications;
- failure behavior;
- concurrency/cancellation implications where applicable;
- deterministic validation that proves it.

Before completion, verify each requirement against the actual implementation.

A passing existing test suite is not evidence for a new requirement unless those tests exercise that requirement.


## 6. Tests

New or changed public behavior requires deterministic behavioral tests.

Tests must exercise the production behavior at the closest practical boundary.

Prefer:

production API/path → controlled dependency → observable result

over:

duplicated implementation → synthetic assertion

Mocks, fakes, and adapters are acceptable when they control real production seams.

A fake must be capable of producing the condition the test claims to verify.

Do not add placeholder, nominal, vacuous, or name-only tests.

Regression tests for a reported defect must fail against the defective behavior and pass because
the defect was actually corrected.

When a task explicitly requires a test matrix or acceptance scenarios, every required scenario
must be represented by meaningful executable coverage.


## 7. Review and revision work

A revision is not a request to patch the review text mechanically.

For every blocking finding:

1. Identify the underlying requirement.
2. Inspect the implementation path responsible for it.
3. Determine why the previous implementation failed the requirement.
4. Correct the production behavior.
5. Add or improve behavioral test coverage when appropriate.
6. Check whether the same defect exists in related paths.
7. Fix only the active blockers; do not run the project's validation/test suite to prove the fix
   — automated review owns validation and will check the result.

Do not repeatedly append narrow patches when the finding exposes a flawed design.

If repeated review failures indicate that the current approach is structurally wrong, reconsider
the implementation instead of continuing the patch loop.


## 8. Validation and evidence

Every change must ultimately pass the repository-configured validation relevant to it, at minimum,
where applicable:

    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test

Ownership of proving this belongs to automated review, not to the implementation or revision
provider session. A coding or revision session must not run the project's validation/test suite,
focused checks, or any other command to prove completion; it implements the requested change within
its specified scope and stops. Automated review selects and runs the task-specific validation
relevant to the changed subsystem, and a failure becomes a blocker for the next revision.

Review's validation evidence must identify the exact revision tested; stale evidence from a
worktree state that has since changed must not be treated as proof for the current revision.

Do not claim a check passed, or narrate validation you did not execute. Clearly distinguish
structural reasoning from validation actually executed.


## 9. Error handling

- Use `Result` for fallible operations.
- Preserve useful error context.
- Do not silently convert failures into success or empty/default state unless that fallback is an
  intentional documented behavior.
- Do not silently recover from corrupted or invalid persisted state.
- Error recovery must leave persistent state consistent.
- Destructive filesystem actions coupled to database mutations must not occur before the database
  operation has safely reached the required state.


## 10. Persistence and lifecycle safety

- Preserve foreign keys and use transactions for multi-step mutations.
- Migrations must be incremental and preserve existing data.
- Preserve foreign-key integrity.
- Treat persisted run, worker, validation, review, lifecycle, change-evidence, and worktree records
  as related lineage rather than isolated rows.
- Multi-step persistent mutations must have defined failure behavior.
- Do not remove recoverable worktrees or artifacts before the corresponding persistent transition
  succeeds.
- Lifecycle transitions must use existing lifecycle/application APIs rather than direct state mutation.


## 11. Concurrency and cancellation

- Keep stateful resources on their valid ownership thread unless the architecture explicitly permits otherwise.
- Use message passing where it provides safer ownership boundaries.
- Cancellation must propagate to the real execution boundary.
- A UI cancellation flag alone is not cancellation.
- Report an operation as cancelled only after execution has reached a defined safe/recoverable boundary.
- Event streaming must deliver events while work is active, not replay them after completion and call it streaming.


## 12. Portability

Orc targets Linux, macOS, and Windows.

- Do not assume Unix behavior in portable application layers.
- Platform-specific code must be isolated.
- Prefer established cross-platform libraries for non-domain platform infrastructure when appropriate.
- Do not claim cross-platform support based solely on validation from one platform.
- Avoid platform-specific shell assumptions in core behavior.
- Do not use unsafe `Send`/`Sync` ownership hacks.


## 13. Dependencies

A new dependency requires a concrete engineering reason.

A dependency is justified when it provides a mature implementation of substantial non-domain
infrastructure and is preferable to maintaining a custom implementation.

Evaluate:

- maintenance activity;
- platform support;
- API stability;
- dependency footprint;
- safety;
- testability;
- fit with Orc's architecture.

"Can be implemented ourselves" is not sufficient reason to reject a dependency.


## 14. Code quality

- Keep code explicit and readable.
- Avoid unnecessary comments.
- Comments should explain non-obvious constraints or decisions, not restate code.
- Do not perform unrelated cleanup during scoped work.
- Do not leave dead experimental architecture after changing approaches.
- Do not suppress compiler or Clippy diagnostics merely to make validation green.
- Avoid `unwrap`/`expect` in production paths where failure can reasonably occur.


## 15. Scope discipline

Workers must:

- stay within the assigned objective;
- inspect supporting modules when necessary to understand the real execution path;
- make supporting changes when they are necessary for correctness;
- avoid unrelated product changes;
- not reinterpret a difficult requirement as optional;
- not silently drop requirements because the existing architecture makes them difficult.

Difficulty is not evidence that a requirement is out of scope.


## 16. Completion standard

Work is complete only when:

- every material task requirement has an implementation;
- required behavioral tests exercise the real behavior;
- persistence and failure paths remain safe;
- the implementation fits the existing architecture or an approved architectural decision;
- no known blocking requirement remains unresolved.

A worker must not describe work as complete while knowingly identifying an unmet task requirement.
Configured project validation is not part of this completion gate: automated review runs it after
the session ends and raises a blocker if it fails.


## 17. Worker completion report

Every coding worker must report:

- files changed;
- behavior changed;
- tests added or changed;
- unresolved risks or limitations;
- architectural decisions requiring approval.

Do not report validation commands as run, or their results, unless they were genuinely executed in
this session for a reason other than proving completion (project validation is not run in this
session at all).

If an architectural decision is required, include:

ORC-ARCHITECTURE-DECISION: <decision>
