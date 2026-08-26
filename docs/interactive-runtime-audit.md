# Interactive runtime architecture audit

This note records the current T-0120/T-0121 implementation and cleanup
decision before Lead-first work. It is a current-state audit, not a redesign.

## KEEP

- **`Runtime` request/event channel and owner thread** — `OrcApp` and its
  rusqlite connection stay on the owner thread. Requests, operation IDs,
  events, and cancellation controls are the production boundary between the
  session and application orchestration.
- **`RuntimePort`** — retained as the editor/session boundary. The editor does
  not know how `OrcApp` is owned, and the boundary supports deterministic
  scripted-runtime behavioral tests. It is not a second orchestration path.
- **Runtime protocol types and editor session state** — requests/events,
  operation tracking, history, selection/confirmation, and bare-command
  routing are shared runtime/session behavior rather than terminal-driver
  details.
- **`main::entry_route` and the Clap command path** — explicit commands remain
  one-shot and bare invocation remains interactive.
- **Fallible `Runtime::submit`** — a disconnected owner is a real runtime
  failure, so returning contextual `Result` is safer than panicking.

## REMOVE NOW

- **Unused `Editor::new_with_runtime`** — duplicated
  `Editor::new(...).with_runtime(...)` and had no callers. It was removed with
  no observable behavior change.

## CONSOLIDATE NOW

- **Editor construction** — runtime attachment now has one construction path,
  `new` followed by `with_runtime`.
- **Routing ownership** — interactive presentation issues `RuntimeRequest`s
  and renders `RuntimeEvent`s; it does not duplicate `OrcApp` business logic or
  direct storage access. One-shot commands retain their existing `main`/`cli`
  path.

## REPLACE IN TERMINAL-LIBRARY TASK

`StdioTerminal` and its platform backends are homemade terminal infrastructure
and should be replaced as a unit by a mature cross-platform terminal library,
without changing the runtime protocol or session ownership boundary. That
replacement should own raw mode and platform console handling, line editing,
history, cursor movement/redraw, key decoding, completion, and
selection/confirmation UI primitives.

The current `Terminal` trait, ANSI redraw, Unix `select`/termios code, Windows
console calls, and byte/key decoder are deliberately not expanded here.
`TerminalStateBackend` remains for failure-safe restoration tests until the
library replacement removes those mechanics.

## Outcome

Only the unused constructor was removed. Runtime orchestration, session
responsibilities, owner-thread message passing, and one-shot compatibility are
preserved; terminal mechanics are explicitly deferred to the library task.
