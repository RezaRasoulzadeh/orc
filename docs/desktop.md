# Desktop application

The desktop application is a Tauri shell with a Vue interface. Build the frontend with `npm install && npm run build`, then use the Tauri CLI for the platform package. It opens an adopted project and exposes dashboard, queue, tasks, agents, runs, review, planning, Lead, approvals, and manual-run actions through the Rust application service.

The desktop and CLI share SQLite state; refresh or reopen the project after CLI changes when needed. Desktop actions still follow the same human review and mutation boundaries as the CLI.

For the v0.2 development preview, the desktop opens the adopted repository containing the Tauri crate's parent directory and `.orc/orc.db`. Run the Tauri development command from the repository checkout; the startup path is resolved from the compiled `src-tauri` manifest location rather than the process working directory. The database must already exist (run `orc init` and `orc adopt` first). If it cannot be opened, startup exits with a readable error.

## Manual provider webviews

Orc opens a manual provider only when its configured workspace URL is an absolute HTTPS URL. Navigation is restricted to the same scheme, host, and port. The webview has no Orc IPC permissions, so provider content cannot invoke Orc commands through the desktop bridge. This is a compatibility boundary, not a claim that Orc controls provider authentication, content, or browser security. Unsupported URLs remain unavailable; use the task packet and CLI submission path instead.
