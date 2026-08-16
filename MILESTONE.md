# Orc v0.1 — Spine

Goal: prove the control loop before connecting any AI provider.

## Flow

CTO command -> Orc state -> EngineeringLeadRequest JSON -> EngineeringLeadResponse JSON -> validation -> task mutation -> persisted state -> status

## Ownership

- CTO: product/architecture authority and approval boundaries.
- Engineering Lead (ChatGPT): protocol architecture, decomposition, integration review.
- Claude Agent 1: CLI + task/state core.
- Claude Agent 2: JSON protocol validation + tests.

## Definition of Done

- `orc init` initializes project state.
- `orc ask "..."` emits valid EngineeringLeadRequest v1 JSON.
- Orc can consume a valid EngineeringLeadResponse v1 and persist resulting tasks.
- Invalid protocol versions/actions are rejected.
- `orc status` shows persisted tasks and pending CTO approvals.
- fmt, clippy with warnings denied, and tests pass.

No real AI integration is part of v0.1.
