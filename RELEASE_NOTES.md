# Orc 0.3.0-beta.1

This is an operator-controlled beta for evaluating Orc's local engineering
workflow. It is not a claim of production-ready autonomy or complete
self-hosting.

## Included in this beta

- Durable task, dependency, run, approval, and lifecycle state backed by the
  project database and isolated task Git worktrees.
- Explicit one-shot Dispatch, semantic Review, Revise, and Accept operations.
  Dispatch never starts Review, a Review PASS remains `acceptance_ready`, and
  acceptance remains an operator action.
- Deterministic validation owned by Orc and kept distinct from semantic Review,
  including exact-worktree freshness and infrastructure-failure reporting.
- Criterion-level semantic Review evidence, durable blocker/revision state, and
  bounded validation-repair and revision lifecycles.
- Provider-independent scheduling and economy tiers with persisted agent,
  model, reasoning-effort, quota, escalation, usage, and context attribution.
- A global agent registry with explicit provider authentication boundaries,
  project attachment, capabilities, actions, permissions, and execution
  profiles.
- Canonical `OrcApp`, lifecycle, and `ProjectOperations` APIs shared by the CLI,
  TUI, and desktop presentation clients.
- Self-hosting identity, worktree, validation-fingerprint, and recursive-process
  guards. These expose readiness; they do not imply unattended self-hosting.
- `orc tui`, an initial terminal queue/task-detail client with explicit
  Dispatch, Review, Revise, and Accept keys derived from canonical next-step
  state.
- A Tauri/Vue desktop client aligned with canonical lifecycle, validation,
  criterion evidence, economy resolution, and self-hosting readiness state.

## Known beta limitations

- Semantic Review is model-assisted and is not proven perfect; deterministic
  validation passing does not guarantee semantic correctness.
- There is no unattended daemon. Provider operations are explicit and
  long-running TUI/desktop actions are currently synchronous.
- The TUI intentionally omits task creation, agent configuration, manual-run
  submission, full diffs/transcripts, and Lead/planning/workflow controls.
- Desktop task evidence is compact, cross-client changes require explicit
  refresh outside existing run events, and specialized recovery remains in the
  Runs view or CLI.
- Self-hosting guards establish safe execution boundaries, not complete
  autonomous Orc-on-Orc development.
- Provider credentials, authentication, live quota, and provider-side context
  accounting remain external to Orc.

Use the source and native packages as beta artifacts. Back up `.orc/orc.db` and
its SQLite sidecar files together before experimenting with important projects.
