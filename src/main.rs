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
        "# Agent Usage\n\nHeimr stores the stable inputs for one unit of agent work. It does not invoke agents, resolve context, or manage sandboxing.\n\n1. Create a workspace with `heimr new <workspace>`.\n2. Set its work brief with `heimr work set <workspace> --from <file>`, starting from `heimr template build` or `heimr template review`. `work set` replaces any existing `WORK.md`.\n3. Prepare its detached repository worktree with `heimr repo prepare <workspace> --from <checkout>` or clone one with `heimr repo prepare <workspace> --url <git-url>`.\n4. Once a worker or orchestrator needs to push the prepared repository somewhere, attach an `origin` push remote with `heimr repo set-push-remote <workspace> --url <url>`. `repo prepare` always strips any clone-origin remote to keep the worktree self-contained, so this is the supported way to (re)attach one; it treats the URL as an opaque string (any forge, any scheme) and is safe to run again with a different URL, which just updates `origin` in place.\n5. Create a dispatch, add its inputs, then seal it.\n6. An orchestrator can consume its sealed handoff with `heimr dispatch handoff <workspace> <dispatch>`.\n7. Launch the sandboxed worker with `heimr preamble build` or `heimr preamble review` as the harness invocation preamble (for example, one `gardr run start --harness-arg` value).\n8. Run `heimr check <workspace>` before consuming a sealed dispatch.\n\nWorkspace commands default to `~/.heimr`; `--root <path>` and `HEIMR_ROOT` override it. `work set` replaces `WORK.md` whether or not one already exists; sealed dispatch inputs remain immutable through Heimr. Sealing writes a mutable `HANDOFF.json` and a SHA-256 inventory of the immutable inputs in `dispatch.json`; `check` verifies that inventory. `dispatch handoff` verifies the inputs before printing the current `HANDOFF.json`.\n\nHeimr owns the dispatch contract: what each agent role is told (`preamble`, `template`), and how one agent's handoff reaches the next (inbox files and PR threads, below). `preamble` and `template` add no new context-delivery mechanism and read no workspace state; they are text generation only. Both require a `build` or `review` kind and work without a workspace root. `preamble <kind>` prints the standard sandboxed-worker preamble: it names `/workspace/WORK.md`, the sole sealed dispatch directory under `/workspace/dispatches`, that dispatch's `HANDOFF.json`, and the forge-access rule (no Skald or Rata, direct `gh`/`az`, no `mcp__tools`). `preamble review` does not claim the reviewer is read-only on the forge: a reviewer posts PR review comments, it just must not modify the target repository or worktree.

`template <kind>` prints a WORK.md scaffold with `{{{{token}}}}` placeholders that the caller (styrir) substitutes literally; unknown tokens are left as-is and no other templating happens. `template build` tokens: `title`, `ticket_id`, `tracker`, `acceptance_criteria`, `branch`, `trunk`, `verify`, `pr_command`, `pr` (empty on the first build of a ticket). `template review` tokens: `title`, `ticket_id`, `acceptance_criteria`, `branch`, `trunk`, `pr`. `build` covers goal, acceptance criteria, constraints, verification, deliverable, escalation, inbox, and PR threads. `review` covers frame (with an explicit read-only boundary on the worktree and ticket), review checklist, review pass (a thorough first pass vs. a follow-up pass that verifies earlier findings against the interdiff, detected from earlier reviews on the PR), PR review instructions, and the expected HANDOFF.json shape.

## Inbox convention

A build dispatch may contain `inbox/*.handoff.json` (a prior dispatch's `HANDOFF.json`, copied
byte-for-byte) and `inbox/*.md` (human notes). The build template tells the agent that every file
under `inbox/` is work addressed to it, to be read before acting. Review dispatches never carry an
inbox, and the review template never mentions one.

## PR threads

A reviewer posts exactly one `COMMENT` review on the PR with one inline thread per new finding, each
thread body prefixed `**<severity>**`; it never approves or requests changes on the forge itself.
On a follow-up pass it replies on the original thread of any earlier finding that is still
unaddressed, rather than opening a duplicate.
A builder reads every unresolved thread on its PR before acting, replies pointing at the commit
that addresses it or declines it in one line, then resolves the thread. A human thread with no
severity tag is treated as should-fix. The PR wins over the inbox when the two conflict.

## Handoff v1

Both shapes carry `\"version\":1`. A build handoff is `{{version:1, status: complete|partial|escalated, branch, commit, pr, summary, acceptance_criteria[{{criterion, status: done|partial|not-started, evidence}}], blockers[], escalation, threads[{{id, url, action: fixed|declined, commit, reply}}]}}`. A review handoff is `{{version:1, status, verdict: approve|request-changes, summary, acceptance_criteria[{{criterion, status: done|partial|missing, evidence}}], findings[{{severity: blocking|should-fix|nit, description, path, line, thread_url}}]}}`. `threads` and `thread_url` are optional additions; everything else is the shape already read by consumers today."
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
        "review" => {
            "Review the dispatched task. Do not modify the target repository or worktree; post findings as PR review comments instead."
        }
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
# {{title}}\n\n\
## Goal\n\
Implement skald ticket {{ticket_id}} in this repository so that every acceptance criterion below holds.\n\
Tracker: {{tracker}}\n\n\
## Acceptance criteria\n\
{{acceptance_criteria}}\n\n\
## Constraints\n\
- Change only this repository. A missing fact is a blocker to report in HANDOFF.json, not something to guess.\n\
- Commit on branch `{{branch}}`, based off `{{trunk}}`; push it to `origin`.\n\
- No plans, notes, or summaries in the repository: the diff is the deliverable.\n\
- Commit subjects: `type(scope): description`, body wrapped at 72 columns, referencing {{ticket_id}}.\n\n\
## Verification\n\
```sh\n\
{{verify}}\n\
```\n\
All of it must pass before the handoff is marked complete.\n\n\
## Deliverable\n\
- Branch `{{branch}}` pushed to origin.\n\
- A draft PR opened with {{pr_command}} (body ≤ 10 lines: one sentence, ≤ 4 bullets, one \"Verified:\" line).\n\
- HANDOFF.json kept current, shape:\n\
  `{\"version\":1,\"status\":\"complete\"|\"partial\"|\"escalated\",\"branch\":\"…\",\"commit\":\"…\",\"pr\":\"<url>|null\",\"summary\":\"…\",\"acceptance_criteria\":[{\"criterion\":\"…\",\"status\":\"done\"|\"partial\"|\"not-started\",\"evidence\":\"…\"}],\"blockers\":[\"…\"],\"escalation\":\"…\"|null,\"threads\":[{\"id\":\"…\",\"url\":\"…\",\"action\":\"fixed\"|\"declined\",\"commit\":\"…\"|null,\"reply\":\"…\"}]}`\n\n\
## Escalation\n\
Try to resolve blockers yourself first. If a blocker cannot be resolved within this task's scope —\n\
the acceptance criteria assume something untrue, a decision belongs to a human, a tool or network\n\
policy makes a criterion impossible here — stop, commit and push what is sound, and set\n\
`\"status\":\"escalated\"` with `\"escalation\"` stating the problem and the decision or change needed.\n\
The ticket goes back to refining for a human; do not keep retrying or narrow the criteria yourself.\n\n\
## Inbox\n\
This dispatch may contain `inbox/*.handoff.json` (a prior dispatch's HANDOFF.json, byte-for-byte)\n\
and `inbox/*.md` (human notes). Treat every file under `inbox/` as work addressed to you: read all\n\
of it before acting.\n\n\
## PR threads\n\
Before acting, read every unresolved review thread on {{pr}}. For each, reply pointing at the\n\
commit that addresses it, or decline it in one line, then resolve the thread. If the PR and the\n\
inbox disagree, the PR wins. Record what you did as `threads[]` in HANDOFF.json.\n";

const REVIEW_WORK_TEMPLATE: &str = "\
# {{title}} (review)\n\n\
## Frame\n\
Branch `{{branch}}` implements skald ticket {{ticket_id}}. Review the worktree at the branch tip\n\
against trunk `{{trunk}}` and the acceptance criteria below. The worktree and the ticket are\n\
read-only: do not modify the target repository or worktree, and do not change ticket state.\n\n\
## Acceptance criteria\n\
{{acceptance_criteria}}\n\n\
## Review checklist\n\
- Design / readability / correctness: fits the existing architecture, idiomatic, no correctness bugs.\n\
- Production safety: failure modes, partial failure, concurrency, bad data, observability, rollback.\n\
- AC fit: map each criterion to done / partial / missing; flag scope drift.\n\
- Severity: blocking / should-fix / nit, each tied to `path:line`. Documentation findings are nits\n\
  unless grossly misleading. A nit is genuinely optional.\n\
- Verdict follows severity mechanically: approve only when there is nothing above nit.\n\n\
## Review pass\n\
Start by reading every earlier review on {{pr}}, with its threads and the replies to them. Resolved\n\
threads count too. If there is no earlier review, this is a **first pass**; otherwise it is a\n\
**follow-up pass**. The last-reviewed revision is the commit the latest earlier review was made on.\n\n\
**First pass: be thorough.** Review the whole diff against `{{trunk}}` and report every finding at\n\
every severity now. Aim for zero new findings on this code in later rounds. Before you post, check\n\
the findings as a set: no two findings may conflict, and no finding may ask for something the\n\
acceptance criteria rule out.\n\n\
**Follow-up pass: verify, don't re-review.** Work from the interdiff\n\
(`git diff <last-reviewed>..HEAD`). If that commit is not in the worktree, use\n\
`git range-diff`, and say so in the summary. Do these three things only:\n\
1. Resolve each earlier finding as addressed, not addressed, or pushed back. Accept a pushback\n\
   unless it is factually wrong or leaves a correctness or safety bug.\n\
2. Check the fix hunks for regressions. Do not ask whether you would have written them another way.\n\
3. Treat settled code as closed. Do not reverse or re-argue an earlier finding, and do not raise\n\
   new findings on code that the first pass reviewed and that hasn't changed. If a requested fix was\n\
   made as asked, leave it, even if you now prefer something else. It is a nit at most, and only a\n\
   real bug justifies asking to revert it.\n\n\
Follow-up exceptions:\n\
- A *blocking* correctness or security bug that the first pass missed is still raised. Start its\n\
  description with `missed in first pass`.\n\
- A substantial change that no earlier finding asked for gets a first-pass review, limited to that\n\
  code. Examples are new files, new behavior, or a rewrite larger than the finding requested. A\n\
  hunk that maps to no earlier finding is the signal.\n\n\
A follow-up pass approves when every earlier finding is addressed or its pushback is accepted, and\n\
the delta adds nothing above nit.\n\n\
## PR review\n\
Post exactly one `COMMENT` review on {{pr}} with one inline thread per new finding, each thread body\n\
starting `**<severity>**`. If an earlier finding is still not addressed, reply on its original\n\
thread. Do not open a duplicate. Report it in HANDOFF.json with that thread's URL.\n\
Never approve or request changes on the forge itself — the verdict belongs in HANDOFF.json.\n\
Record each finding's `thread_url` in HANDOFF.json.\n\n\
## Deliverable\n\
HANDOFF.json kept current, shape:\n\
`{\"version\":1,\"status\":\"complete\",\"verdict\":\"approve\"|\"request-changes\",\"summary\":\"…\",\"acceptance_criteria\":[{\"criterion\":\"…\",\"status\":\"done\"|\"partial\"|\"missing\",\"evidence\":\"…\"}],\"findings\":[{\"severity\":\"blocking\"|\"should-fix\"|\"nit\",\"description\":\"…\",\"path\":\"…\",\"line\":0,\"thread_url\":\"…\"}]}`\n";
