# M10 interface usability audit

**Audit basis:** current repository revision `e41bef45880b882706a4bbae1617cf843e67a96a` (`Advance task index to M09-007`).

**Scope:** compare the implemented CLI, Rust TUI, Tauri command bridge, and Vue desktop adapter against the canonical `OrcApp`, `ProjectOperations`, read-model, workflow, Controller, and M08 APIs. This audit evaluates operational information hierarchy where it affects a user's ability to understand or advance work. It does not evaluate visual styling.

## Conclusions

- The CLI has the broadest complete workflow coverage. It exposes project initialization/adoption, queue and task operations, dependencies, dispatch and agent selection, review and revision, acceptance/rejection/cancellation/requeue, plans and Lead decisions, approvals, run submission, history, agent governance, and economy inspection. Its project model is the current repository (`.`), rather than the desktop registry's multi-project switcher.
- The TUI is intentionally narrow. It provides queue and task detail, refresh and navigation, dispatch, automated semantic review, revision through the previous implementation agent, and accept. It does not expose project switching, task creation/editing, dependency mutation, run history/detail, worktree/diff inspection, rejection, cancellation, requeue, Controller recommendations, Plan/Lead, approvals, agent governance, economy detail, or M08 curation.
- Tauri exposes broader canonical operations than the Vue UI currently wires. The command bridge includes project registry management, agent onboarding/configuration/governance, manual-run submission, planning, approvals, reports, task actions, run detail/logs, Lead proposals, and the full desktop read-model surface. Vue wires much of that surface, but leaves several command capabilities unused or only partially represented.
- Missing Controller recommendation/authorization/execution routes and M08 curation routes are adapter exposure gaps. The canonical Rust APIs already exist in `OrcApp`, `controller_actions`, and the capability-specific M08 curation modules.
- General task metadata editing after creation is the only clear shared application API gap. Rust has narrow post-creation updates for required capabilities, scope, context files, and expected changes; it has no general update seam for title, objective, role, or priority. The desktop adapters therefore cannot provide a complete task editor without a new shared API.
- Existing dispatch, revision, recovery, run, validation, and read-model APIs are sufficient for the first TUI normal-task lifecycle slice. No prerequisite API change is required for that slice.
- M09-007 remains blocked because the measured canonical M08 source is empty. Specialization/backend selection remains deferred. Experience curation remains explicit and supervised only.

## Coverage matrix

The labels in the final column distinguish the implementation state:

- **Exists:** the operation is implemented and usable through that surface.
- **Adapter gap:** the canonical Rust operation exists, but the surface has no route or does not wire the available route.
- **Shared seam:** the application API needed for a complete operation is absent.
- **Partial:** the surface exposes useful read or write coverage, but not the complete workflow operation.

| Workflow | CLI | TUI | Tauri backend | Vue desktop | Assessment |
|---|---|---|---|---|---|
| Project/open/switch | `init`/`adopt` operate on the current checkout; no registry switch command | Starts against current checkout | Registered/imported/adopted projects can be opened, closed, relocated, removed | `ProjectPicker` supports import, adopt, open/switch, close, relocate, remove | Desktop project registry exists; CLI/TUI are current-project surfaces |
| Queue/tasks | `queue`, `task list`, `task show` | Queue list and task detail | `snapshot`, `queue`, `tasks`, `task_operations`, `task_details` | Dashboard and Tasks workspace with filters and details | Exists; shared read models are in use |
| Create/edit | Full create; narrow edits for scope, capabilities, context, expected changes | No create/edit | Create only; narrow edit commands absent | Create only; narrow edit commands absent | General title/objective/role/priority edit is a shared seam; other desktop edits are adapter gaps |
| Dependencies | `task depend`/`undepend`; details show dependency state | Displays dependencies and blockers | `task_action` supports add/remove | Add/remove dependency controls are wired | Exists in CLI/Tauri/Vue; TUI is read-only |
| Dispatch/agent selection | `dispatch --agent`, `schedule`, queue dispatch, deterministic selection explanations | Dispatches with canonical default selection | `dispatch` accepts optional agent; `task_action` accepts agent for dispatch/revision | Dispatch uses canonical default selection; no task-level agent picker | Canonical selection exists; explicit TUI/Vue override controls are adapter gaps |
| Live runs | Run listing plus manual run submit/patch/fail | No run workspace | Runs, workspace, run details, worker log, lifecycle event forwarding | Runs workspace, live event refresh, output/logs and run detail | Exists in CLI/Tauri/Vue; TUI adapter gap |
| Worktree/diff | Task worktree/diff and review diff/file views | No worktree path or diff view | Review and run-review payloads include worktree/change evidence | Task “inspect diff” and captured run change evidence | Canonical Git/review evidence exists; TUI exposure gap |
| Validation | Validation is run by canonical dispatch/review/manual patch paths and shown in task/review output | Shows validation state and selected commands | Run details serialize validation reports | Task and run views show validation state, commands, and diagnostics | Functionality exists as evidence-producing workflow; no separate ad hoc rerun control is required for the first slice |
| Review | Manual inspection, automated review, project review, diff, history, full JSON | Automated semantic review | `review`, `review_run`, and `task_action(review)` | Task review and run review/evidence views | Exists; TUI has the intended narrow automated path |
| Revision | Revision with explicit agent or configured selection | Revision through previous implementation agent | `task_action(revise)` supports explicit agent or previous agent | Revision feedback is wired; no explicit agent selection | Canonical revision paths exist; TUI/Vue explicit-selection gap is adapter-level |
| Accept/reject/cancel/requeue | All four task lifecycle operations | Accept only | `task_action` supports all four | Accept and cancel are wired; reject is absent; requeue is wired only for failed-run recovery | Missing Vue reject and general requeue controls are adapter gaps |
| Blockers/recovery | Task show, requeue, non-convergence unblock, failure output | Displays blockers; no recovery action | Read models expose blockers; task actions expose requeue | Failed-run recovery requeues eligible tasks; task details display blockers | Recovery APIs exist; TUI recovery controls and richer recovery inspection are adapter gaps |
| Controller recommendations/authorization | No generic typed Controller action route | None | No command for `propose_controller_action`, `authorize_controller_action`, or `execute_authorized_controller_action` | Lead conversation/proposals are exposed, but the typed Controller action boundary is not | Adapter exposure gap; canonical recommendation and authorization/execution APIs exist |
| Plan/Lead | Plan request/run/revise/apply, Lead run/review/pending/history/resolve/apply/consume | None | Planning request/validation/apply and Lead context/proposals/invoke/apply/reject | Lead and Planner workspaces, proposal apply/reject, plan validate/apply | Exists broadly in CLI/Tauri/Vue; TUI adapter gap |
| Approvals | List and resolve | None | List and resolve commands | Approval list and resolve controls | Exists in CLI/Tauri/Vue; TUI adapter gap |
| Agent configuration/governance | Onboard with explicit approval, attach/detach, permissions, enablement, availability, priority, profile, model/effort, quota, action profiles | None | Registry, onboarding, configuration, action profiles, settings, quota sync, manual workspace commands | Register/archive, enablement, priority/profile/model/effort, action toggles, manual provider workspace | Canonical and bridge coverage is broad; Vue does not wire onboarding, permissions, availability/quota governance, or all configuration fields |
| History/failures | Workflow history, review history, runs, failure/recovery output | None beyond current task summary | Runs, lifecycle activity, worker log, review evidence, failure fields | Runs timeline, output, worker log, captured evidence, failure state | Exists in CLI/Tauri/Vue; TUI adapter gap |
| Token/context/economy | `economy show/context/configure`; task show includes invocation, context, and token evidence | Shows only latest economy resolution in task summary | Snapshot/task/run read models carry economy and token/context evidence, but no dedicated invocation command | Dashboard/task/run views show aggregate tokens and selected economy/context fields | Canonical evidence exists; TUI and some detailed desktop views are partial adapters |
| Supervised M08 experience curation | No curation command | None | No M08 command | No M08 command | Adapter exposure gap; explicit, capability-local Rust curation APIs exist |

## Canonical API and adapter evidence

The shared application layer already centralizes the normal task lifecycle: `OrcApp::dispatch`, `automated_review`, `revise`, `revise_with_previous_agent`, `accept`, `reject`, `cancel`, and `requeue` are implemented in `src/app.rs:2439-2570`. `ProjectOperations` provides queue, task summaries/details, action legality, run summaries, economy summaries, and recovery legality in `src/operations.rs:537-938`. The TUI already consumes these read models and invokes the normal lifecycle operations in `src/tui/mod.rs:15-132`.

The CLI command surface is defined in `src/main.rs:86-402` and task-specific operations in `src/cli/task.rs:7-112`. It includes direct task metadata operations, dependency operations, review/diff/worktree views, plan/Lead and approval commands, workflow history, and agent/run/economy subcommands. This is the broadest complete operator surface.

The TUI state machine has only `Queue` and `Detail` screens and only `Dispatch`, `Review`, `Revise`, and `Accept` actions in `src/tui/state.rs:9-50` and `src/tui/state.rs:243-253`. Its detail view renders dependencies, blockers, validation, review criteria, economy, and self-hosting readiness in `src/tui/ui.rs:186-296`, so the first normal lifecycle slice can be completed by extending existing controls and refresh behavior rather than changing the core API.

The Tauri bridge registers project, task, agent, planning, approval, report, run, review, Lead, and manual-run commands in `src-tauri/src/lib.rs:650-952` and `src-tauri/src/lib.rs:1168-1234`. The TypeScript adapter declares most of these in `src/lib/api.ts:135-187`. The Vue application wires project selection, tasks, runs, agents, Lead, Planner, approvals, reports, project settings, and selected task actions in `src/App.vue:151-320` and `src/App.vue:326-413`. The bridge is consequently broader than the currently wired Vue workflows.

The typed Controller action boundary is present in `src/controller_actions.rs:309-370`, with application composition and supervised continuation in `src/app.rs:1987-2151`. No Tauri command, TypeScript API method, Vue action, or TUI action exposes this recommendation/authorization/execution sequence. This is an adapter gap, not a new kernel seam.

M08 remains a distinct, explicitly curated dataset. `OrcApp` exposes create/list/inventory/snapshot/retire and capability-local curation methods in `src/app.rs:64-245`; the M08 modules validate typed inputs and project them into the canonical record. No presentation adapter currently exposes those methods. The curation path must remain operator-selected and supervised; no automatic harvesting or inference from ordinary workflow traffic is implied by this audit.

Post-creation task editing is narrower. `OrcApp` exposes scope, required capabilities, context, and expected-change updates in `src/app.rs:2913-2939`, while storage implements those fields in `src/storage/db.rs:8825-8859`. There is no corresponding general update for title, objective, role, or priority. That missing application seam should be treated as a focused follow-on instead of being re-created independently in each adapter.

## M09 and scope constraints

`docs/orc-next/STATUS.md` records M09-007 as planned and blocked because the canonical M08 source was measured empty. `docs/orc-next/ROADMAP.md` keeps M09 as the current Controller-specialization milestone, defers backend selection, and defines M10 as interface integration. No interface work should fabricate source examples, select a trainer/backend, train a candidate, alter model weights, or change the default model. M08 curation remains an explicit supervised authority and is not an automatic runtime side effect.

## Smallest M10 implementation sequence

1. Complete the normal task lifecycle slice in the TUI using the existing `ProjectOperations` read models and `OrcApp` operations: queue/detail, canonical dispatch, execution refresh, validation/review evidence, revision through the previous implementation agent, and accept. Keep legality and mutation in the existing application methods. No prerequisite API change is required.
2. Add the next TUI operational controls that are already supported by Rust: run/history and worktree/diff inspection, then reject, cancel, requeue, dependency changes, and explicit agent-selection/recovery views as each control becomes necessary.
3. Add typed Tauri commands and Vue adapters for Controller recommendation inspection plus explicit authorization/execution. Preserve the existing deterministic legality and authorization boundary; the UI must display the proposal and authorization result before mutation.
4. Add supervised M08 inventory, snapshot, and capability-local curation views through the existing `OrcApp` methods. Require explicit verification/outcome/provenance input and keep persistence on the canonical M08 path.
5. Add one shared task metadata update API for title, objective, role, and priority, then wire it into Tauri/Vue and the TUI task editor. This is the only identified shared application seam.
6. Close remaining desktop parity gaps for agent onboarding/governance, detailed economy/context inspection, and Plan/Lead workflow controls. Keep specialization/backend selection and model changes outside M10.

The sequence keeps the first deliverable small and usable while preserving canonical Rust ownership of facts, legality, authorization, persistence, validation, and workflow transitions.
