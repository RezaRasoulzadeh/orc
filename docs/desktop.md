# Desktop application

The desktop application is a Tauri shell with a Vue interface. Build the frontend with `npm install && npm run build`, then use `cargo tauri build` for the platform package. It opens an adopted project and exposes dashboard, queue, tasks, agents, runs, review, planning, Lead, approvals, and manual-run actions through the Rust application service.

## Installation and launcher

Install both artifacts from a release build. `orc --ui` checks beside the CLI first, followed by these supported locations:

- Linux: beside the CLI, `$HOME/.local/lib/orc/orc-desktop`, `/usr/local/lib/orc/orc-desktop`, or `/usr/lib/orc/orc-desktop`.
- macOS: `/Applications/Orc.app/Contents/MacOS/orc-desktop` or the corresponding per-user Applications path.
- Windows: `%LOCALAPPDATA%\\Programs\\Orc\\orc-desktop.exe`.

Run `orc --ui` to start the installed desktop application. It detaches the process, gives it no standard input/output/error, and returns to the terminal immediately. It never runs Vite or a Tauri development server. If the desktop package is missing, the error lists the searched locations and gives an installation action.

The supported Linux and macOS user install builds release artifacts, validates that Tauri produced an installable package, and installs both components:

```sh
./scripts/install.sh
```

The default Linux prefix is `$HOME/.local`: the CLI goes to `$HOME/.local/bin`, the desktop executable to `$HOME/.local/lib/orc`, and the application-menu entry and icon to `$HOME/.local/share`. For a system-style install, run `sudo env PREFIX=/usr ./scripts/install.sh`; this installs the exact `/usr/bin` and `/usr/lib/orc` layout searched by the launcher. `PREFIX=/usr/local` is also supported. Ensure the chosen `bin` directory is on `PATH`. On macOS, the CLI is installed under `$HOME/.local/bin` and the application bundle under `$HOME/Applications`.

On Windows, run:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/install.ps1
```

This installs both executables under `%LOCALAPPDATA%\Programs\Orc` and adds that directory to the user `PATH`. Re-run the same installer from a newer checkout to upgrade. Uninstall with `./scripts/install.sh --uninstall` or `powershell -ExecutionPolicy Bypass -File scripts/install.ps1 --uninstall`. Project databases are not removed.

Release validation uses `npm run tauri:build` followed by `npm run validate:package`. The build selects deterministic native bundle formats (`deb` and `rpm`, `app` and `dmg`, or `msi` and NSIS), and validation fails unless both the release desktop executable and a native Tauri installer or application bundle exist under `src-tauri/target/release`.

The desktop and CLI share SQLite state; refresh or reopen the project after CLI changes when needed. Desktop actions still follow the same human review and mutation boundaries as the CLI.

For v0.3.0, the desktop starts with no project open and does not depend on the process working directory. Its startup path is resolved from the compiled `src-tauri` manifest location; project state is opened only after a registered project is selected. A moved project remains registered as unavailable until relocated. Removing a project removes only its registry entry; re-importing it preserves the existing `.orc/orc.db` state. Normal lifecycle actions use desktop controls; raw protocol JSON is limited to advanced disclosures. The database must already exist (run `orc init` and `orc adopt` first).

## Manual provider webviews

Orc opens a manual provider only when its configured workspace URL is an absolute HTTPS URL. Navigation is restricted to the same scheme, host, and port. The webview has no Orc IPC permissions, so provider content cannot invoke Orc commands through the desktop bridge. This is a compatibility boundary, not a claim that Orc controls provider authentication, content, or browser security. Unsupported URLs remain unavailable; use the task packet and CLI submission path instead.
