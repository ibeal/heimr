# Heimr

Heimr is a durable, tool-agnostic CLI for agent workspaces. It stores work and dispatch inputs; it does not invoke agents, resolve contexts, or manage sandboxing.

Workspaces are stored in `~/.heimr` by default. Set `HEIMR_ROOT`, or pass `--root <directory>` to override it.

Run `heimr help` to list commands. `heimr docs` prints the agent-facing workspace lifecycle and its invariants; neither command requires a workspace root.

```sh
heimr --root /workspaces new ask-2026-09-02
heimr --root /workspaces work set ask-2026-09-02 --from work.md
heimr --root /workspaces repo prepare ask-2026-09-02 --from /checkout
# or clone a remote repository
heimr --root /workspaces repo prepare ask-2026-09-02 --url https://github.com/example/project.git
heimr --root /workspaces dispatch new ask-2026-09-02 build
heimr --root /workspaces dispatch put ask-2026-09-02 build --path AGENTS.md --from build-agents.md
heimr --root /workspaces dispatch seal ask-2026-09-02 build
heimr --root /workspaces dispatch handoff ask-2026-09-02 build
```

`work set` writes the one stable `WORK.md` and refuses replacement. `repo prepare` creates a detached Git worktree in `repository/` from the supplied checkout and is safe to repeat after validation. Repository guidance such as `repository/AGENTS.md` remains there.

`dispatch put` takes file content from `--from` or standard input and only accepts confined relative paths. A dispatch accepts any files before sealing. `dispatch seal` writes its file inventory and SHA-256 digests to `dispatch.json` and adds that dispatch's `HANDOFF.json`; sealed dispatches are immutable through Heimr. `dispatch handoff <workspace> <dispatch>` prints a sealed dispatch's verified `HANDOFF.json`, so an orchestrator need not discover the workspace path or read it directly. It rejects unknown or unsealed dispatches. `heimr check <workspace>` validates the workspace and every sealed dispatch inventory.

```text
<root>/<workspace>/
├── WORK.md
├── repository/                 # prepared checkout, including repository/AGENTS.md
└── dispatches/
    └── <dispatch>/
        ├── AGENTS.md           # optional, dispatch-specific input
        ├── HANDOFF.json        # created by seal
        └── dispatch.json       # inventory + SHA-256 digests
```

Gardr can mount the workspace broadly, mount `repository/` beneath the execution root, and overlay the selected dispatch's `AGENTS.md` at that root. This preserves repository-local guidance while each dispatch supplies its own top-level instructions and handoff.

## Dispatch ergonomics: preamble and template

`heimr preamble (build|review)` and `heimr template (build|review)` remove per-ticket
orchestration boilerplate. Neither needs a workspace root; both read no workspace state, so they
add no new context-delivery mechanism, they only save typing the same fixed text every dispatch.
This is an interim Heimr home for the two; a future Styrir interface is expected to own it.

`heimr preamble <kind>` prints the standard harness-invocation preamble for a sealed dispatch,
suitable as one `gardr run start --harness-arg` value:

```
$ heimr preamble build
Read /workspace/WORK.md and the sole sealed dispatch directory under /workspace/dispatches before acting. Your cwd is already the repository. Keep the dispatch HANDOFF.json current. You have no Skald or Rata. Use gh or az directly for forge access if authorized; do not use mcp__tools. Build the dispatched task.
```

It names the fixed sandbox paths a worker reads (`/workspace/WORK.md`, the sole sealed dispatch
directory under `/workspace/dispatches`, and that dispatch's `HANDOFF.json`) and the forge-access
boundary (no Skald or Rata; direct `gh`/`az`; no `mcp__tools`). `heimr preamble review` prints the
same preamble with a read-only closing instruction instead. All task-specific content still comes
from `WORK.md` and the sealed dispatch itself, per the sandboxed-worker contract; the preamble
carries no dispatch-specific detail because a Gardr-mounted sandbox always exposes exactly one
sealed dispatch at those fixed paths.

`heimr template <kind>` prints a `WORK.md` scaffold to fill in and pass to
`heimr work set --from <file>`:

- `build` — goal, acceptance criteria, constraints, verification, deliverable.
- `review` — frame, acceptance criteria, review checklist, an explicit read-only boundary, and
  the expected `HANDOFF.json` shape.

Both templates and the preamble are documentation and text generation only: they do not read or
write a workspace, and sealed dispatch immutability is unaffected.
