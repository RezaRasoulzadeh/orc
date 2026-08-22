# Manual-agent workflow

Register an agent with `--mode manual`, then dispatch it. Orc creates a waiting external run and prints a task packet. Give that packet to the human or external provider. Complete it with `orc run submit RUN_ID --file OUTPUT`, or submit a validated Git patch with `orc run submit-patch RUN_ID PATCH`. Use `-` for stdin. Record an unsuccessful attempt with `orc run fail RUN_ID "reason"`.

The desktop application exposes the same waiting runs and actions. A configured manual provider may also be opened in an embedded HTTPS webview; see [desktop security boundaries](../desktop.md#manual-provider-webviews).
