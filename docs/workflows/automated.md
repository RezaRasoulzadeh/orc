# Automated-agent workflow

Register a supported automated agent (`codex`, `copilot`, or `antigravity`), configure its profile where required, and verify it with `orc agent show ID` and `orc doctor`. Copilot uses its CLI-managed credentials and supports model/effort settings, but not Orc's structured-output, Lead, or quota-sync capabilities. Use `orc schedule TASK_ID --explain` to inspect selection, then `orc dispatch TASK_ID` or `orc dispatch-queue --concurrency 2`.

Orc creates a task worktree, runs the backend with the repository contract and task packet, and records output, validation, result metadata, and lifecycle events. Inspect `orc runs`, `orc review TASK_ID`, `orc task diff TASK_ID`, and `orc task worktree TASK_ID`. Apply feedback with `orc revise`; accept, reject, requeue, or cancel only after inspecting the result.
