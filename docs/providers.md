# Provider contracts

Provider-specific behavior is implemented behind `ProviderAdapter` and `Worker`. Orc's
orchestration, lifecycle, persistence, validation, and review paths do not branch on a provider
after selecting that adapter.

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
