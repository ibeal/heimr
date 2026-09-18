use std::env;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use heimr::{Workspace, list_workspaces, read_input};

fn main() {
    if let Err(error) = run() {
        eprintln!("heimr: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let root = option(&mut args, "--root").map(PathBuf::from);
    if args.is_empty() {
        return Err(usage());
    }
    let command = args.remove(0);
    match command.as_str() {
        "help" => {
            reject_extra(&args)?;
            print_help();
            return Ok(());
        }
        "docs" => {
            reject_extra(&args)?;
            print_docs();
            return Ok(());
        }
        "preamble" => {
            let kind = kind_arg(&args)?;
            print!("{}", harness_preamble(kind)?);
            return Ok(());
        }
        "template" => {
            let kind = kind_arg(&args)?;
            print!("{}", work_template(kind)?);
            return Ok(());
        }
        _ => {}
    }
    let root = root
        .or_else(|| env::var_os("HEIMR_ROOT").map(PathBuf::from))
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".heimr")))
        .ok_or_else(|| "--root, HEIMR_ROOT, or HOME is required".to_owned())?;
    match command.as_str() {
        "new" => {
            let name = one(&args)?;
            Workspace::open(&root, name)?.create()?;
        }
        "list" => {
            for name in list_workspaces(&root)? {
                println!("{name}");
            }
        }
        "path" => println!("{}", Workspace::open(&root, one(&args)?)?.root.display()),
        "show" => {
            let workspace = Workspace::open(&root, one(&args)?)?;
            workspace.check()?;
            println!("workspace={}", workspace.root.display());
            println!("work={}", workspace.work_path().display());
            println!("repository={}", workspace.repository_path().display());
            for dispatch in
                std::fs::read_dir(workspace.dispatches_path()).map_err(|error| error.to_string())?
            {
                let dispatch = dispatch.map_err(|error| error.to_string())?;
                if dispatch
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_dir()
                {
                    println!("dispatch={}", dispatch.file_name().to_string_lossy());
                }
            }
        }
        "check" => Workspace::open(&root, one(&args)?)?.check()?,
        "work" => {
            require_word(&mut args, "set")?;
            let name = take(&mut args)?;
            let from = option(&mut args, "--from").map(PathBuf::from);
            reject_extra(&args)?;
            Workspace::open(&root, &name)?.set_work(&read_input(from.as_deref())?)?;
        }
        "repo" => repo(&root, args)?,
        "dispatch" => dispatch(&root, args)?,
        _ => return Err(usage()),
    }
    Ok(())
}

fn repo(root: &Path, mut args: Vec<String>) -> Result<(), String> {
    let action = take(&mut args)?;
    let name = take(&mut args)?;
    let workspace = Workspace::open(root, &name)?;
    match action.as_str() {
        "prepare" => {
            let from = option(&mut args, "--from");
            let url = option(&mut args, "--url");
            reject_extra(&args)?;
            match (from, url) {
                (Some(from), None) => workspace.prepare_repository(Path::new(&from)),
                (None, Some(url)) => workspace.prepare_repository_url(&url),
                _ => Err("exactly one of --from or --url is required".to_owned()),
            }
        }
        "set-push-remote" => {
            let url = option(&mut args, "--url").ok_or_else(|| "--url is required".to_owned())?;
            reject_extra(&args)?;
            workspace.set_push_remote(&url)
        }
        _ => Err(usage()),
    }
}

fn dispatch(root: &Path, mut args: Vec<String>) -> Result<(), String> {
    let action = take(&mut args)?;
    let workspace_name = take(&mut args)?;
    let dispatch_name = take(&mut args)?;
    let workspace = Workspace::open(root, &workspace_name)?;
    match action.as_str() {
        "new" => {
            reject_extra(&args)?;
            workspace.new_dispatch(&dispatch_name)
        }
        "put" => {
            let path =
                option(&mut args, "--path").ok_or_else(|| "--path is required".to_owned())?;
            let from = option(&mut args, "--from").map(PathBuf::from);
            reject_extra(&args)?;
            workspace.put_dispatch_file(
                &dispatch_name,
                Path::new(&path),
                &read_input(from.as_deref())?,
            )
        }
        "seal" => {
            reject_extra(&args)?;
            println!("{}", workspace.seal_dispatch(&dispatch_name)?.display());
            Ok(())
        }
        "handoff" => {
            reject_extra(&args)?;
            io::stdout()
                .write_all(&workspace.handoff(&dispatch_name)?)
                .map_err(|error| error.to_string())?;
            Ok(())
        }
        _ => Err(usage()),
    }
}

fn option(args: &mut Vec<String>, flag: &str) -> Option<String> {
    args.iter()
        .position(|value| value == flag)
        .and_then(|index| {
            args.remove(index);
            (index < args.len()).then(|| args.remove(index))
        })
}
fn take(args: &mut Vec<String>) -> Result<String, String> {
    if args.is_empty() {
        Err(usage())
    } else {
        Ok(args.remove(0))
    }
}
fn one(args: &[String]) -> Result<&str, String> {
    if args.len() == 1 {
        Ok(&args[0])
    } else {
        Err(usage())
    }
}
fn require_word(args: &mut Vec<String>, word: &str) -> Result<(), String> {
    if take(args)? == word {
        Ok(())
    } else {
        Err(usage())
    }
}
fn reject_extra(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(format!("unexpected arguments: {}", args.join(" ")))
    }
}
fn usage() -> String {
    "usage: heimr [--root <path>] <command> ...\n\nTry `heimr help` for commands or `heimr docs` for agent guidance.".to_owned()
}

fn print_help() {
    println!(
        "Heimr manages durable, tool-agnostic agent workspaces.\n\nUsage: heimr [--root <path>] <command> ...\n\nCommands:\n  new <workspace>\n  list\n  path <workspace>\n  show <workspace>\n  check <workspace>\n  work set <workspace> [--from <path>]\n  repo prepare <workspace> (--from <checkout> | --url <git-url>)\n  repo set-push-remote <workspace> --url <url>\n  dispatch new <workspace> <dispatch>\n  dispatch put <workspace> <dispatch> --path <path> [--from <path>]\n  dispatch seal <workspace> <dispatch>\n  dispatch handoff <workspace> <dispatch>\n  preamble (build|review)\n  template (build|review)\n  docs\n  help\n\nWorkspace commands default to ~/.heimr. Set HEIMR_ROOT or pass --root to override it.\n\n`preamble` and `template` take no workspace root: they print the standard sandboxed-worker harness preamble and the matching WORK.md scaffold for a build or review dispatch."
    );
}

fn print_docs() {
    println!(
        "# Agent Usage\n\nHeimr stores the stable inputs for one unit of agent work. It does not invoke agents, resolve context, or manage sandboxing.\n\n1. Create a workspace with `heimr new <workspace>`.\n2. Set its immutable work brief with `heimr work set <workspace> --from <file>`, starting from `heimr template build` or `heimr template review`.\n3. Prepare its detached repository worktree with `heimr repo prepare <workspace> --from <checkout>` or clone one with `heimr repo prepare <workspace> --url <git-url>`.\n4. Once a worker or orchestrator needs to push the prepared repository somewhere, attach an `origin` push remote with `heimr repo set-push-remote <workspace> --url <url>`. `repo prepare` always strips any clone-origin remote to keep the worktree self-contained, so this is the supported way to (re)attach one; it treats the URL as an opaque string (any forge, any scheme) and is safe to run again with a different URL, which just updates `origin` in place.\n5. Create a dispatch, add its inputs, then seal it.\n6. An orchestrator can consume its sealed handoff with `heimr dispatch handoff <workspace> <dispatch>`.\n7. Launch the sandboxed worker with `heimr preamble build` or `heimr preamble review` as the harness invocation preamble (for example, one `gardr run start --harness-arg` value).\n8. Run `heimr check <workspace>` before consuming a sealed dispatch.\n\nWorkspace commands default to `~/.heimr`; `--root <path>` and `HEIMR_ROOT` override it. `WORK.md` and sealed dispatch inputs are immutable through Heimr. Sealing writes a mutable `HANDOFF.json` and a SHA-256 inventory of the immutable inputs in `dispatch.json`; `check` verifies that inventory. `dispatch handoff` verifies the inputs before printing the current `HANDOFF.json`.\n\n`preamble` and `template` are an interim home for dispatch-ergonomics scaffolding pending a dedicated Styrir interface; they add no new context-delivery mechanism and read no workspace state. Both require a `build` or `review` kind and work without a workspace root. `preamble <kind>` prints the standard sandboxed-worker preamble: it names `/workspace/WORK.md`, the sole sealed dispatch directory under `/workspace/dispatches`, that dispatch's `HANDOFF.json`, and the forge-access rule (no Skald or Rata, direct `gh`/`az`, no `mcp__tools`). `template <kind>` prints a WORK.md scaffold: `build` covers goal, acceptance criteria, constraints, verification, and deliverable; `review` covers frame, acceptance criteria, review checklist, an explicit read-only boundary, and the expected HANDOFF.json shape."
    );
}

fn kind_arg(args: &[String]) -> Result<&str, String> {
    match one(args)? {
        kind @ ("build" | "review") => Ok(kind),
        _ => Err("kind must be build or review".to_owned()),
    }
}

/// The standard harness-invocation preamble for a sealed dispatch. It is
/// meant to be supplied verbatim as one `gardr run start --harness-arg`
/// value: it names the fixed sandbox context paths a worker reads
/// (`/workspace/WORK.md`, the sole sealed dispatch directory, and its
/// `HANDOFF.json`) and the forge-access boundary (no Skald or Rata, direct
/// `gh`/`az`, no `mcp__tools`). It carries no workspace- or dispatch-specific
/// detail, since a Gardr-mounted sandbox always exposes exactly one sealed
/// dispatch at those fixed paths; all task-specific context still comes from
/// the dispatch itself, not from this preamble.
fn harness_preamble(kind: &str) -> Result<String, String> {
    let action = match kind {
        "build" => "Build the dispatched task.",
        "review" => "Review the dispatched task. Do not modify the target repository or worktree.",
        _ => return Err("kind must be build or review".to_owned()),
    };
    Ok(format!(
        "Read /workspace/WORK.md and the sole sealed dispatch directory under /workspace/dispatches before acting. Your cwd is already the repository. Keep the dispatch HANDOFF.json current. You have no Skald or Rata. Use gh or az directly for forge access if authorized; do not use mcp__tools. {action}\n"
    ))
}

/// A WORK.md scaffold matching the schema for `kind`. `build` and `review`
/// dispatches read different things out of WORK.md, so each gets its own
/// template; filling one in and passing it to `heimr work set --from <file>`
/// is the whole workflow this exists to save.
fn work_template(kind: &str) -> Result<String, String> {
    match kind {
        "build" => Ok(BUILD_WORK_TEMPLATE.to_owned()),
        "review" => Ok(REVIEW_WORK_TEMPLATE.to_owned()),
        _ => Err("kind must be build or review".to_owned()),
    }
}

const BUILD_WORK_TEMPLATE: &str = "\
# <Task title>\n\n\
## Goal\n\
<What must be true when this task is done, and why it matters.>\n\n\
## Acceptance criteria\n\
- <Criterion 1>\n\
- <Criterion 2>\n\n\
## Constraints\n\
<Anything the worker must not do, or must do a specific way: scope limits,\n\
forbidden approaches, required conventions.>\n\n\
## Verification\n\
<Exact commands to run and what passing looks like: test suites, lint,\n\
formatting, manual checks.>\n\n\
## Deliverable\n\
<What the worker must leave behind: a pushed branch, a PR, an updated\n\
HANDOFF.json with per-criterion evidence, and anything else expected.>\n";

const REVIEW_WORK_TEMPLATE: &str = "\
# <Task title> (review)\n\n\
## Frame\n\
<What was built and why, and the revision or diff under review. Do not\n\
include the builder's prompt, journal, or handoff reasoning; the reviewer\n\
works from the target revision and this brief alone.>\n\n\
## Acceptance criteria\n\
- <Criterion 1>\n\
- <Criterion 2>\n\n\
## Review checklist\n\
- <Design, readability, correctness>\n\
- <Production safety>\n\
- <Fit against each acceptance criterion above>\n\n\
## Read-only boundary\n\
The reviewer must not modify the target repository or worktree, and must not\n\
resolve the builder's PR threads or Skald state. Findings only.\n\n\
## Expected HANDOFF.json shape\n\
Record an outcome, blocking/should-fix/nit findings with `path:line` evidence,\n\
and a verdict per acceptance criterion. Approve only when there are no\n\
blocking or should-fix findings.\n";
