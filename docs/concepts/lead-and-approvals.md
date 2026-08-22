# Lead, planning, and approvals

`orc plan-request` and `orc discovery-request` are read-only structured request protocols. Their responses are validated before persistence. A plan is applied only by `orc apply-plan` after human review.

The Engineering Lead receives project context and returns a message plus proposals for plans, tasks, revisions, or approval requests. Proposals are recorded with `pending`, `applying`, `applied`, or `rejected` status. The Lead runs read-only and cannot mutate the repository or dispatch work. A human must explicitly apply or reject each mutation proposal.

Approval requests are durable records, listed with `orc approvals list` and resolved with `orc approvals resolve ID`. Resolving an approval records the decision; it does not imply that an unrelated patch or architectural change was applied.
