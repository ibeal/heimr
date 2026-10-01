# Heimr

Heimr is a durable, tool-agnostic CLI for agent workspaces. It stores work, curated agent inputs, source selection, state, and legacy dispatch data; it does not invoke agents, resolve contexts, or manage sandboxing.

Workspaces are stored in `~/.heimr` by default. Set `HEIMR_ROOT`, or pass `--root <directory>` to override it.

Run `heimr help` to list commands. `heimr docs` prints the agent-facing workspace lifecycle and its invariants; neither command requires a workspace root.

```sh
# Define a reusable curated environment under /workspaces/.templates/rust:
heimr --root /workspaces workspace-template new rust
heimr --root /workspaces workspace-template environment agents rust --from AGENTS.md
heimr --root /workspaces workspace-template environment task rust --from task.md
heimr --root /workspaces workspace-template environment prompt rust --from prompt.md
heimr --root /workspaces workspace-template environment context rust --path INDEX.md --from rata-output.md
# Every workspace starts as an independent copy of its selected template:
heimr --root /workspaces new ask-2026-09-02 --template rust
heimr --root /workspaces work set ask-2026-09-02 --from work.md
# Target an existing directory without copying it:
heimr --root /workspaces source mount ask-2026-09-02 --from /checkout
# Or create a self-contained managed clone:
heimr --root /workspaces repo prepare ask-2026-09-02 --url https://github.com/example/project.git
# Optional per-workspace override after copying the template:
heimr --root /workspaces environment prompt ask-2026-09-02 --from task-specific-prompt.md
heimr --root /workspaces mount-plan ask-2026-09-02
heimr --root /workspaces dispatch new ask-2026-09-02 build
heimr --root /workspaces dispatch put ask-2026-09-02 build --path AGENTS.md --from build-agents.md
heimr --root /workspaces dispatch seal ask-2026-09-02 build
heimr --root /workspaces dispatch handoff ask-2026-09-02 build
```

`work set` writes `WORK.md`, replacing any existing one; sealed dispatch inputs remain immutable. `repo prepare` creates a detached Git worktree in `repository/` from the supplied checkout and is safe to repeat after validation. Repository guidance such as `repository/AGENTS.md` remains there.

`dispatch put` takes file content from `--from` or standard input and only accepts confined relative paths. A dispatch accepts any files before sealing. `dispatch seal` writes its immutable input inventory and SHA-256 digests to `dispatch.json` and adds that dispatch's mutable `HANDOFF.json`; `HANDOFF.json` is excluded from the inventory so workers can report progress. `dispatch handoff <workspace> <dispatch>` verifies the sealed inputs, then prints its current handoff, so an orchestrator need not discover the workspace path or read it directly. It rejects unknown or unsealed dispatches. `heimr check <workspace>` validates the workspace and every sealed dispatch inventory.

```text
<root>/<workspace>/
├── WORK.md                     # retained work brief
├── repository/                 # managed clone, when selected
├── dispatches/                 # retained legacy dispatches and handoffs
├── environment/
│   ├── AGENTS.md
│   ├── task.md
│   ├── prompt.md
│   ├── context/INDEX.md
│   └── manifest.json           # curated-input SHA-256 inventory
├── state/HANDOFF.json
└── workspace.json              # source identity/revision and environment digests
```

Named template workspaces live below `<root>/.templates/`. `new` copies the selected template's entire confined `environment/` tree and records its name and digests; later template edits cannot mutate an existing workspace. If no template is named, Heimr materializes and uses the documented blank `default` template. `workspace-template check` validates a template before use.

`mount-plan` validates and prints the machine-readable Gardr/Styrir boundary: the selected existing directory or managed clone is writable `/repo`, `environment/` is read-only `/agent`, and `state/` is writable `/agent-state`. The plan never mounts `workspace.json` or `environment/manifest.json` as writable. `heimr check` verifies template shape, source metadata, confinement, digests, the state handoff, repositories, and legacy dispatch inventories.

`workspace-template environment` populates reusable inputs; `environment agents`, `task`, and `prompt` replace the copied files for one workspace. Both forms support `context --path <relative>` only below `context/`; paths cannot escape. Inputs can be caller files or output captured from existing Rata commands. `heimr migrate <workspace>` adds the curated directories and metadata to a legacy workspace without changing `WORK.md`, `repository/`, `dispatches/`, or legacy handoffs; repeating it is safe.

## Dispatch contract: preamble, template, inbox, PR threads

Heimr owns the dispatch contract: what each agent role is told, and how one agent's handoff
reaches the next. `heimr preamble (build|review)` and `heimr template (build|review)` remove
per-ticket orchestration boilerplate. Neither needs a workspace root; both read no workspace
state, so they add no new context-delivery mechanism — they only save typing the same fixed text
every dispatch.

`heimr preamble <kind>` prints the standard harness-invocation preamble for a sealed dispatch,
suitable as one `gardr run start --harness-arg` value:

```
$ heimr preamble build
Read /workspace/WORK.md and the sole sealed dispatch directory under /workspace/dispatches before acting. Your cwd is already the repository. Keep the dispatch HANDOFF.json current. You have no Skald or Rata. Use gh or az directly for forge access if authorized; do not use mcp__tools. Build the dispatched task.
```

It names the fixed sandbox paths a worker reads (`/workspace/WORK.md`, the sole sealed dispatch
directory under `/workspace/dispatches`, and that dispatch's `HANDOFF.json`) and the forge-access
boundary (no Skald or Rata; direct `gh`/`az`; no `mcp__tools`). `heimr preamble review` prints the
same preamble with a closing instruction not to modify the target repository or worktree; it does
not claim the reviewer is read-only on the forge, since a reviewer posts PR review comments. All
task-specific content still comes from `WORK.md` and the sealed dispatch itself, per the
sandboxed-worker contract; the preamble carries no dispatch-specific detail because a
Gardr-mounted sandbox always exposes exactly one sealed dispatch at those fixed paths.

`heimr template <kind>` prints a `WORK.md` scaffold with `{{token}}` placeholders that the caller
(styrir) substitutes literally, to pass to `heimr work set --from <file>`. Unknown tokens are left
as-is; there is no other templating.

- `build` tokens: `title`, `ticket_id`, `tracker`, `acceptance_criteria`, `branch`, `trunk`,
  `verify`, `pr_command`, `pr` (empty on the first build). Covers goal, acceptance criteria,
  constraints, verification, deliverable, escalation, inbox, and PR threads.
- `review` tokens: `title`, `ticket_id`, `acceptance_criteria`, `branch`, `trunk`, `pr`. Covers
  frame (with an explicit read-only boundary on the worktree and ticket), review checklist, review
  pass (a thorough first pass, or a follow-up pass that verifies earlier findings against the
  interdiff; detected from earlier reviews on the PR), PR review instructions, and the expected `HANDOFF.json` shape.

**Inbox.** A build dispatch may carry `inbox/*.handoff.json` (a prior dispatch's `HANDOFF.json`,
byte-for-byte) and `inbox/*.md` (human notes); the build template tells the agent every file under
`inbox/` is work addressed to it. Review dispatches never carry an inbox, and the review template
never mentions one.

**PR threads.** A reviewer posts exactly one `COMMENT` review with one inline thread per new finding,
each body prefixed `**<severity>**`, and never approves or requests changes on the forge itself. On
a follow-up pass it replies on the original thread of a still-unaddressed finding instead. A
builder reads every unresolved thread on its PR before acting, replies pointing at a commit or
declines in one line, then resolves it. The PR wins over the inbox on conflict.

**Handoff v1**, documented in full by `heimr docs`: a build handoff carries
`{version, status, branch, commit, pr, summary, acceptance_criteria[], blockers[], escalation,
threads[]}`; a review handoff carries `{version, status, verdict, summary, acceptance_criteria[],
findings[]}`, each finding optionally carrying a `thread_url`.

Templates, preamble, inbox, and PR threads are documentation and text generation only: they do not
read or write a workspace, and sealed dispatch immutability is unaffected.
