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
        "repo" => {
            require_word(&mut args, "prepare")?;
            let name = take(&mut args)?;
            let from = option(&mut args, "--from");
            let url = option(&mut args, "--url");
            reject_extra(&args)?;
            let workspace = Workspace::open(&root, &name)?;
            match (from, url) {
                (Some(from), None) => workspace.prepare_repository(Path::new(&from))?,
                (None, Some(url)) => workspace.prepare_repository_url(&url)?,
                _ => return Err("exactly one of --from or --url is required".to_owned()),
            }
        }
        "dispatch" => dispatch(&root, args)?,
        _ => return Err(usage()),
    }
    Ok(())
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
        "Heimr manages durable, tool-agnostic agent workspaces.\n\nUsage: heimr [--root <path>] <command> ...\n\nCommands:\n  new <workspace>\n  list\n  path <workspace>\n  show <workspace>\n  check <workspace>\n  work set <workspace> [--from <path>]\n  repo prepare <workspace> (--from <checkout> | --url <git-url>)\n  dispatch new <workspace> <dispatch>\n  dispatch put <workspace> <dispatch> --path <path> [--from <path>]\n  dispatch seal <workspace> <dispatch>\n  dispatch handoff <workspace> <dispatch>\n  docs\n  help\n\nWorkspace commands default to ~/.heimr. Set HEIMR_ROOT or pass --root to override it."
    );
}

fn print_docs() {
    println!(
        "# Agent Usage\n\nHeimr stores the stable inputs for one unit of agent work. It does not invoke agents, resolve context, or manage sandboxing.\n\n1. Create a workspace with `heimr new <workspace>`.\n2. Set its immutable work brief with `heimr work set <workspace> --from <file>`.\n3. Prepare its detached repository worktree with `heimr repo prepare <workspace> --from <checkout>` or clone one with `heimr repo prepare <workspace> --url <git-url>`.\n4. Create a dispatch, add its inputs, then seal it.\n5. An orchestrator can consume its sealed handoff with `heimr dispatch handoff <workspace> <dispatch>`.\n6. Run `heimr check <workspace>` before consuming a sealed dispatch.\n\nWorkspace commands default to `~/.heimr`; `--root <path>` and `HEIMR_ROOT` override it. `WORK.md` and every sealed dispatch are immutable through Heimr. Sealing writes `HANDOFF.json` and a SHA-256 inventory in `dispatch.json`; `check` verifies that inventory. `dispatch handoff` prints the sealed `HANDOFF.json` only after verifying that dispatch's inventory."
    );
}
