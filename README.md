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
