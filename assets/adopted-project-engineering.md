# Engineering Contract

This is the default contract for work performed in an adopted repository. The
repository's maintainers may replace or extend it with project-specific rules.
Task instructions may specialize this contract but must not silently weaken it.

## Scope and architecture

- Stay within the task's explicit objective, acceptance criteria, context, and expected changes.
- Inspect and reuse the existing repository's architecture, conventions, dependencies, and public APIs.
- Do not redesign unrelated systems or generalize for hypothetical requirements.
- Do not modify unrelated files or behavior.
- Add a dependency only when the task requires it and the maintenance cost is justified.
- If the task requires an architectural or product decision that is not in the contract, stop and report `ORC-ARCHITECTURE-DECISION: <decision and reason>`.

## Correctness and tests

- Implement the observable requirement, not a superficial test-specific workaround.
- New or changed behavior requires focused deterministic tests at the closest practical production boundary.
- Regression tests must fail against the defective behavior and pass because the underlying defect was corrected.
- Preserve useful error context and established failure semantics.
- Keep public API changes as small and compatible as the task permits.

## Validation ownership

- Implementation and revision workers edit the task worktree and return structured completion evidence.
- Workers must not run the repository's configured deterministic validation suite to prove completion.
- Orc runs configured deterministic validation after implementation or revision and before semantic Review.
- Review consumes fresh passing validation evidence for the exact current worktree and judges only the task contract.
- Never claim a check passed when it was not executed by the owning lifecycle stage.

## Safety

- Do not access credentials, secrets, production services, or unrelated external systems.
- Do not perform deployments, publish packages, or make network-dependent changes unless the task explicitly requires and authorizes them.
- Preserve recoverable work and repository history; avoid destructive operations outside the task worktree.
- Do not knowingly report completion with an unmet acceptance criterion or unresolved task requirement.
