# Project lifecycle and ownership

Orc manages a project; Orc is not installed into the project. A project is an
existing repository that is brought under Orc management.

## Init, adopt, and import

`orc init` is the local CLI bootstrap. It creates or opens `.orc/orc.db`, the
Orc-owned database, and creates the initial project record when needed. It is
safe to run before Git adoption and remains supported for existing one-shot
CLI workflows. It does not make a directory a repository or establish Git
identity.

`orc adopt` brings the current Git repository under Orc management. It resolves
the repository root, requires a usable Git repository, initializes the local
database if necessary, creates the project record when necessary, and ensures
the project documents exist. Adoption is idempotent. It never overwrites an
existing project document or `.orc/engineering.md`.

Desktop **Import** is registry-only: it remembers an already initialized and
adopted project so it can be opened by the desktop application. It does not
create `.orc` state and does not alter repository files. Desktop **Adopt** is
the equivalent of `orc adopt`, followed by registering the resulting project.
Relocating a remembered project only updates the desktop registry after the
repository and its database have been verified.

## Project documents

Adoption may create these project-owned documents when they are missing:

| Path | Ownership and purpose |
| --- | --- |
| `.orc/engineering.md` | The mandatory coder/worker contract. It is project-owned and is automatically loaded for coder execution. |
| `.orc/project.md` | Human/discovery project summary. |
| `.orc/architecture.md` | Human/discovery architecture summary. |
| `.orc/roadmap.md` | Human/discovery roadmap and unknowns. |
| `.orc/project-identity.json` | Optional source-controlled durable repository identity. Orc's own repository commits the reserved `dev.orc.orchestrator` identity so self-hosting is path- and remote-independent. |

These files are never overwritten by init, adopt, or import. Discovery updates
the summaries only through its explicit apply operation; it does not replace
the engineering contract.

## Source control and Orc-owned runtime state

Project documents, including `.orc/engineering.md`, belong in source control.
Teams may also commit any deliberately maintained validation configuration
under `.orc/`.

The following remain Orc-owned local runtime state and must stay untracked:

- `.orc/orc.db`, `.orc/orc.db-wal`, and `.orc/orc.db-shm`;
- `.orc/worktrees/` and its task checkouts;
- other generated logs, temporary files, and local registry/credential data.

The database is the authoritative source of persistent Orc project, task,
agent, run, review, and lifecycle state. Git remains authoritative for source
files and the project documents that are intentionally committed.

When the committed project identity recognizes Orc itself, `orc status` and
the project operations snapshot expose self-hosting readiness. Mutation still
uses the normal scheduler and lifecycle. Execution is blocked if the identity
is not valid in both HEAD and the working checkout or if the configured root is
a linked task worktree. Task worktree metadata is verified against the
canonical `.orc/worktrees/<task-id>` checkout before mutation, validation,
review, or acceptance.
