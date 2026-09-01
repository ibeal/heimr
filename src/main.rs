use std::env;
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
    let root = option(&mut args, "--root")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HEIMR_ROOT").map(PathBuf::from))
        .ok_or_else(|| "--root or HEIMR_ROOT is required".to_owned())?;
    if args.is_empty() {
        return Err(usage());
    }
    let command = args.remove(0);
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
            let from =
                option(&mut args, "--from").ok_or_else(|| "--from is required".to_owned())?;
            reject_extra(&args)?;
            Workspace::open(&root, &name)?.prepare_repository(Path::new(&from))?;
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
    "usage: heimr --root <path> <new|list|show|path|check|work|repo|dispatch> ...".to_owned()
}
