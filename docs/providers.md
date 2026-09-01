# Provider contracts

Provider-specific behavior is implemented behind `ProviderAdapter` and `Worker`. Orc's
orchestration, lifecycle, persistence, validation, and review paths do not branch on a provider
after selecting that adapter.

## Codex CLI

Each automated action launches a new `codex exec` process. Orc never calls `resume` or `fork`, and passes `--ephemeral` so unused provider sessions are not persisted. It also passes `--ignore-user-config`: authentication still comes from the agent's isolated `CODEX_HOME`, while personal model defaults, project trust entries, MCP/plugin configuration, and other unrelated profile configuration do not enter the automated runtime. Orc supplies the resolved model and effort explicitly.

Lead, Planner, and semantic Review run read-only in empty non-repository directories. Dispatch, Revision, completion repair, and validation repair retain mutation capability. Their launcher process starts in an isolated directory, while Codex's logical `--cd` and explicit `--add-dir` both point to the task worktree so relative tool paths cannot escape into the main checkout. Codex may therefore discover worktree-level repository instructions; Orc attributes the sizes of those known files.

Orc persists exact packet and top-level section byte/character sizes, truncations, fixed instruction and output-schema sizes, known profile/repository instruction-file sizes, isolation and filesystem-access state, and new-session state. Codex-reported input/cached/output totals are stored beside those facts. Codex does not expose token counts per context source, so provider bootstrap prompts and tool schemas—and any remaining input-token attribution—stay explicitly unknown.

## GitHub Copilot CLI

The Copilot adapter follows GitHub's [programmatic CLI reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-programmatic-reference)
and [CLI command reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference):

- `-p` runs one non-interactive prompt and exits.
- `-s` keeps the captured result to the agent response.
- `--allow-all-tools` permits tool execution required by programmatic use, while
  `--no-ask-user` prevents clarification prompts from blocking an automated run.
- `--model` and `--effort` are translated from Orc's provider execution options.
- `COPILOT_HOME` is used when an agent profile path is configured; credentials remain owned by
  Copilot CLI.

Copilot CLI returns plain text rather than Orc's provider-structured JSON event stream. The adapter
therefore advertises repository read/write, command execution, streaming process output, and
cancellation, but not `structured_output`. Copilot does not implement Orc's read-only Lead
boundary or quota synchronization; those adapter operations return explicit unsupported errors.

Authentication is checked with Copilot's documented `/user list` account command in prompt mode.
See GitHub's [authentication reference](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/authenticate-copilot-cli)
for supported interactive and non-interactive credential sources.
