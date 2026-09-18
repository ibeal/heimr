use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use heimr::Workspace;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
fn temporary_directory() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "heimr-test-{timestamp}-{}",
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).unwrap();
    path
}

#[test]
fn stable_work_and_multiple_dispatches_are_independent() {
    let temp = temporary_directory();
    let workspace = Workspace::open(&temp, "task").unwrap();
    workspace.create().unwrap();
    workspace.set_work(b"do the work\n").unwrap();
    assert!(workspace.set_work(b"replace it").is_err());
    workspace.new_dispatch("build").unwrap();
    workspace.new_dispatch("review").unwrap();
    workspace
        .put_dispatch_file(
            "build",
            PathBuf::from("AGENTS.md").as_path(),
            b"build instructions",
        )
        .unwrap();
    workspace
        .put_dispatch_file(
            "review",
            PathBuf::from("AGENTS.md").as_path(),
            b"review instructions",
        )
        .unwrap();
    workspace.seal_dispatch("build").unwrap();
    fs::write(
        workspace
            .dispatch_path("build")
            .unwrap()
            .join("HANDOFF.json"),
        "{\"status\": \"completed\"}\n",
    )
    .unwrap();
    workspace.check().unwrap();
    assert_eq!(
        fs::read_to_string(
            workspace
                .dispatch_path("build")
                .unwrap()
                .join("HANDOFF.json")
        )
        .unwrap(),
        "{\"status\": \"completed\"}\n"
    );
    assert!(
        workspace
            .put_dispatch_file("build", PathBuf::from("extra").as_path(), b"no")
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(workspace.dispatch_path("review").unwrap().join("AGENTS.md")).unwrap(),
        "review instructions"
    );
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn rejects_escape_paths_and_detects_modified_sealed_content() {
    let temp = temporary_directory();
    let workspace = Workspace::open(&temp, "task").unwrap();
    workspace.create().unwrap();
    workspace.new_dispatch("build").unwrap();
    assert!(
        workspace
            .put_dispatch_file("build", PathBuf::from("../AGENTS.md").as_path(), b"bad")
            .is_err()
    );
    assert!(
        workspace
            .put_dispatch_file("build", PathBuf::from("dispatch.json").as_path(), b"bad")
            .is_err()
    );
    workspace
        .put_dispatch_file(
            "build",
            PathBuf::from("nested/AGENTS.md").as_path(),
            b"good",
        )
        .unwrap();
    workspace
        .put_dispatch_file(
            "build",
            PathBuf::from("nested/HANDOFF.json").as_path(),
            b"tracked",
        )
        .unwrap();
    workspace.seal_dispatch("build").unwrap();
    workspace.check().unwrap();
    fs::write(
        workspace
            .dispatch_path("build")
            .unwrap()
            .join("nested/HANDOFF.json"),
        b"changed",
    )
    .unwrap();
    assert!(
        workspace
            .check()
            .unwrap_err()
            .contains("inventory does not match")
    );
    fs::remove_dir_all(temp).unwrap();
}

#[cfg(unix)]
#[test]
fn rejects_dispatch_paths_through_symlinks() {
    use std::os::unix::fs::symlink;

    let temp = temporary_directory();
    let outside = temporary_directory();
    let workspace = Workspace::open(&temp, "task").unwrap();
    workspace.create().unwrap();
    workspace.new_dispatch("build").unwrap();
    symlink(
        &outside,
        workspace.dispatch_path("build").unwrap().join("escape"),
    )
    .unwrap();
    assert!(
        workspace
            .put_dispatch_file("build", PathBuf::from("escape/AGENTS.md").as_path(), b"bad")
            .unwrap_err()
            .contains("symlink")
    );
    assert!(!outside.join("AGENTS.md").exists());
    fs::remove_dir_all(temp).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn cli_uses_the_configured_root() {
    let root = temporary_directory();
    let output = Command::new(env!("CARGO_BIN_EXE_heimr"))
        .args(["--root", root.to_str().unwrap(), "new", "demo"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert!(root.join("demo").is_dir());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_defaults_to_a_heimr_directory_in_home() {
    let home = temporary_directory();
    let output = Command::new(env!("CARGO_BIN_EXE_heimr"))
        .args(["new", "demo"])
        .env_remove("HEIMR_ROOT")
        .env("HOME", &home)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(home.join(".heimr/demo").is_dir());
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn cli_reads_only_verified_sealed_dispatch_handoffs() {
    let root = temporary_directory();
    let workspace = Workspace::open(&root, "task").unwrap();
    workspace.create().unwrap();
    workspace.new_dispatch("build").unwrap();
    workspace
        .put_dispatch_file(
            "build",
            PathBuf::from("HANDOFF.json").as_path(),
            b"{\"status\": \"ready\"}\n",
        )
        .unwrap_err();
    workspace.seal_dispatch("build").unwrap();
    let record_path = workspace
        .dispatch_path("build")
        .unwrap()
        .join("dispatch.json");
    let record = fs::read_to_string(&record_path).unwrap();
    fs::write(
        record_path,
        record.replacen(
            "  \"files\": [\n",
            "  \"files\": [\n    {\"path\": \"HANDOFF.json\", \"sha256\": \"0d574f6126865166523eca67e8d7003c8b0d4610bcb45778853703e1c1297a6c\"}\n",
            1,
        ),
    )
    .unwrap();

    let binary = env!("CARGO_BIN_EXE_heimr");
    let handoff = Command::new(binary)
        .args([
            "--root",
            root.to_str().unwrap(),
            "dispatch",
            "handoff",
            "task",
            "build",
        ])
        .output()
        .unwrap();
    assert!(handoff.status.success(), "{handoff:?}");
    assert_eq!(
        handoff.stdout,
        b"{\n  \"version\": 1,\n  \"status\": \"pending\"\n}\n"
    );

    workspace.new_dispatch("unsealed").unwrap();
    let unsealed = Command::new(binary)
        .args([
            "--root",
            root.to_str().unwrap(),
            "dispatch",
            "handoff",
            "task",
            "unsealed",
        ])
        .output()
        .unwrap();
    assert!(!unsealed.status.success());
    assert!(
        String::from_utf8(unsealed.stderr)
            .unwrap()
            .contains("dispatch is not sealed: unsealed")
    );

    let missing = Command::new(binary)
        .args([
            "--root",
            root.to_str().unwrap(),
            "dispatch",
            "handoff",
            "task",
            "missing",
        ])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(
        String::from_utf8(missing.stderr)
            .unwrap()
            .contains("dispatch does not exist: missing")
    );

    fs::write(
        workspace
            .dispatch_path("build")
            .unwrap()
            .join("HANDOFF.json"),
        b"{\"status\": \"altered\"}\n",
    )
    .unwrap();
    let altered = Command::new(binary)
        .args([
            "--root",
            root.to_str().unwrap(),
            "dispatch",
            "handoff",
            "task",
            "build",
        ])
        .output()
        .unwrap();
    assert!(altered.status.success(), "{altered:?}");
    assert_eq!(altered.stdout, b"{\"status\": \"altered\"}\n");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_prepares_a_detached_repository_from_a_url() {
    let temp = temporary_directory();
    let source = temp.join("source");
    let remote = temp.join("remote.git");
    run_git(&temp, ["init", source.to_str().unwrap()]);
    run_git(&source, ["config", "user.email", "heimr@example.test"]);
    run_git(&source, ["config", "user.name", "Heimr Test"]);
    fs::write(source.join("README.md"), "source\n").unwrap();
    run_git(&source, ["add", "README.md"]);
    run_git(&source, ["commit", "-m", "initial"]);
    run_git(
        &temp,
        [
            "clone",
            "--bare",
            source.to_str().unwrap(),
            remote.to_str().unwrap(),
        ],
    );

    let binary = env!("CARGO_BIN_EXE_heimr");
    assert!(
        Command::new(binary)
            .args(["--root", temp.to_str().unwrap(), "new", "task"])
            .status()
            .unwrap()
            .success()
    );
    let prepared = Command::new(binary)
        .args([
            "--root",
            temp.to_str().unwrap(),
            "repo",
            "prepare",
            "task",
            "--url",
            remote.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(prepared.status.success(), "{prepared:?}");
    let repository = temp.join("task/repository");
    assert_eq!(
        fs::read_to_string(repository.join("README.md")).unwrap(),
        "source\n"
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repository)
            .args(["symbolic-ref", "-q", "HEAD"])
            .status()
            .unwrap()
            .code()
            .is_some_and(|code| code != 0)
    );

    assert!(
        Command::new(binary)
            .args(["--root", temp.to_str().unwrap(), "new", "failed"])
            .status()
            .unwrap()
            .success()
    );
    let failed = Command::new(binary)
        .args([
            "--root",
            temp.to_str().unwrap(),
            "repo",
            "prepare",
            "failed",
            "--url",
            temp.join("missing.git").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(
        String::from_utf8(failed.stderr)
            .unwrap()
            .contains("git clone failed")
    );
    assert!(!temp.join("failed/repository").exists());
    assert!(fs::read_dir(temp.join("failed")).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".repository-clone-")
    }));

    let invalid = Command::new(binary)
        .args([
            "--root",
            temp.to_str().unwrap(),
            "repo",
            "prepare",
            "failed",
            "--from",
            source.to_str().unwrap(),
            "--url",
            remote.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(
        String::from_utf8(invalid.stderr)
            .unwrap()
            .contains("exactly one of --from or --url is required")
    );

    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn set_push_remote_attaches_origin_after_preparing_from_a_local_checkout() {
    let temp = temporary_directory();
    let source = temp.join("source");
    run_git(&temp, ["init", source.to_str().unwrap()]);
    run_git(&source, ["config", "user.email", "heimr@example.test"]);
    run_git(&source, ["config", "user.name", "Heimr Test"]);
    fs::write(source.join("README.md"), "source\n").unwrap();
    run_git(&source, ["add", "README.md"]);
    run_git(&source, ["commit", "-m", "initial"]);

    let workspace = Workspace::open(&temp, "task").unwrap();
    workspace.create().unwrap();
    workspace.prepare_repository(&source).unwrap();

    let repository = workspace.repository_path();
    assert!(git_stdout(&repository, ["remote"]).is_empty());

    workspace
        .set_push_remote("https://example.test/team/project.git")
        .unwrap();
    assert_eq!(
        git_stdout(&repository, ["remote", "get-url", "origin"]),
        "https://example.test/team/project.git"
    );

    // Setting it again with a different URL updates the existing remote
    // instead of failing because origin already exists, and works
    // identically for a different-shaped forge URL (no scheme special-casing).
    workspace
        .set_push_remote("https://dev.azure.com/org/project/_git/repo")
        .unwrap();
    assert_eq!(
        git_stdout(&repository, ["remote", "get-url", "origin"]),
        "https://dev.azure.com/org/project/_git/repo"
    );
    assert_eq!(git_stdout(&repository, ["remote"]), "origin");

    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn set_push_remote_attaches_origin_after_preparing_from_a_url() {
    let temp = temporary_directory();
    let source = temp.join("source");
    let remote = temp.join("remote.git");
    run_git(&temp, ["init", source.to_str().unwrap()]);
    run_git(&source, ["config", "user.email", "heimr@example.test"]);
    run_git(&source, ["config", "user.name", "Heimr Test"]);
    fs::write(source.join("README.md"), "source\n").unwrap();
    run_git(&source, ["add", "README.md"]);
    run_git(&source, ["commit", "-m", "initial"]);
    run_git(
        &temp,
        [
            "clone",
            "--bare",
            source.to_str().unwrap(),
            remote.to_str().unwrap(),
        ],
    );

    let workspace = Workspace::open(&temp, "task").unwrap();
    workspace.create().unwrap();
    workspace
        .prepare_repository_url(remote.to_str().unwrap())
        .unwrap();

    let repository = workspace.repository_path();
    assert!(git_stdout(&repository, ["remote"]).is_empty());

    workspace
        .set_push_remote("https://github.com/example/repo.git")
        .unwrap();
    assert_eq!(
        git_stdout(&repository, ["remote", "get-url", "origin"]),
        "https://github.com/example/repo.git"
    );

    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn set_push_remote_fails_clearly_when_the_repository_has_not_been_prepared() {
    let temp = temporary_directory();
    let workspace = Workspace::open(&temp, "task").unwrap();
    workspace.create().unwrap();

    let error = workspace
        .set_push_remote("https://example.test/team/project.git")
        .unwrap_err();
    assert!(error.contains("repository does not exist"), "{error}");
    assert!(!workspace.repository_path().exists());

    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn cli_sets_and_updates_the_push_remote() {
    let temp = temporary_directory();
    let source = temp.join("source");
    run_git(&temp, ["init", source.to_str().unwrap()]);
    run_git(&source, ["config", "user.email", "heimr@example.test"]);
    run_git(&source, ["config", "user.name", "Heimr Test"]);
    fs::write(source.join("README.md"), "source\n").unwrap();
    run_git(&source, ["add", "README.md"]);
    run_git(&source, ["commit", "-m", "initial"]);

    let binary = env!("CARGO_BIN_EXE_heimr");
    assert!(
        Command::new(binary)
            .args(["--root", temp.to_str().unwrap(), "new", "task"])
            .status()
            .unwrap()
            .success()
    );

    let not_prepared = Command::new(binary)
        .args([
            "--root",
            temp.to_str().unwrap(),
            "repo",
            "set-push-remote",
            "task",
            "--url",
            "https://example.test/team/project.git",
        ])
        .output()
        .unwrap();
    assert!(!not_prepared.status.success());
    assert!(
        String::from_utf8(not_prepared.stderr)
            .unwrap()
            .contains("repository does not exist")
    );

    assert!(
        Command::new(binary)
            .args([
                "--root",
                temp.to_str().unwrap(),
                "repo",
                "prepare",
                "task",
                "--from",
                source.to_str().unwrap(),
            ])
            .status()
            .unwrap()
            .success()
    );

    let set = Command::new(binary)
        .args([
            "--root",
            temp.to_str().unwrap(),
            "repo",
            "set-push-remote",
            "task",
            "--url",
            "https://example.test/team/project.git",
        ])
        .output()
        .unwrap();
    assert!(set.status.success(), "{set:?}");

    let repository = temp.join("task/repository");
    assert_eq!(
        git_stdout(&repository, ["remote", "get-url", "origin"]),
        "https://example.test/team/project.git"
    );

    let update = Command::new(binary)
        .args([
            "--root",
            temp.to_str().unwrap(),
            "repo",
            "set-push-remote",
            "task",
            "--url",
            "https://dev.azure.com/org/project/_git/repo",
        ])
        .output()
        .unwrap();
    assert!(update.status.success(), "{update:?}");
    assert_eq!(
        git_stdout(&repository, ["remote", "get-url", "origin"]),
        "https://dev.azure.com/org/project/_git/repo"
    );

    fs::remove_dir_all(temp).unwrap();
}

fn run_git<const N: usize>(directory: &std::path::Path, args: [&str; N]) {
    let status = Command::new("git")
        .current_dir(directory)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success());
}

fn git_stdout<const N: usize>(directory: &std::path::Path, args: [&str; N]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// Asserts that `repository` supports ordinary Git history and diff
/// operations, and that none of its Git administrative state resolves
/// outside the repository itself (the portability contract this module
/// exists to guarantee).
fn assert_self_contained_worktree(repository: &std::path::Path) {
    run_git(repository, ["log", "--oneline"]);
    run_git(repository, ["show", "HEAD"]);
    run_git(repository, ["branch", "-a"]);
    std::fs::write(repository.join("portability.txt"), "changed\n").unwrap();
    run_git(repository, ["diff", "--stat"]);
    std::fs::remove_file(repository.join("portability.txt")).unwrap();

    let git_dir = git_stdout(repository, ["rev-parse", "--absolute-git-dir"]);
    let git_dir = std::fs::canonicalize(git_dir).unwrap();
    let repository = std::fs::canonicalize(repository).unwrap();
    assert!(
        git_dir.starts_with(&repository),
        "git-dir {} escapes repository {}",
        git_dir.display(),
        repository.display()
    );

    let common_dir = git_stdout(&repository, ["rev-parse", "--git-common-dir"]);
    let common_dir = std::fs::canonicalize(repository.join(&common_dir))
        .or_else(|_| std::fs::canonicalize(&common_dir))
        .unwrap();
    assert_eq!(
        common_dir, git_dir,
        "repository is a linked worktree, not a self-contained clone"
    );

    let remotes = git_stdout(&repository, ["remote"]);
    assert!(
        remotes.is_empty(),
        "prepared repository retained remotes: {remotes}"
    );
}

#[test]
fn prepared_repository_from_a_branch_tip_is_self_contained_after_the_source_is_removed() {
    let temp = temporary_directory();
    let source = temp.join("source");
    run_git(&temp, ["init", source.to_str().unwrap()]);
    run_git(&source, ["config", "user.email", "heimr@example.test"]);
    run_git(&source, ["config", "user.name", "Heimr Test"]);
    fs::write(source.join("README.md"), "branch tip\n").unwrap();
    run_git(&source, ["add", "README.md"]);
    run_git(&source, ["commit", "-m", "initial"]);
    run_git(&source, ["checkout", "-b", "feature"]);
    fs::write(source.join("README.md"), "branch tip, updated\n").unwrap();
    run_git(&source, ["commit", "-am", "update"]);

    let workspace = Workspace::open(&temp, "task").unwrap();
    workspace.create().unwrap();
    workspace.prepare_repository(&source).unwrap();

    let repository = workspace.repository_path();
    assert_eq!(
        fs::read_to_string(repository.join("README.md")).unwrap(),
        "branch tip, updated\n"
    );
    // A self-contained clone survives the source checkout disappearing
    // entirely, which a linked worktree could never do.
    fs::remove_dir_all(&source).unwrap();
    assert_self_contained_worktree(&repository);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn prepared_repository_from_a_detached_commit_is_self_contained_after_the_source_is_removed() {
    let temp = temporary_directory();
    let source = temp.join("source");
    run_git(&temp, ["init", source.to_str().unwrap()]);
    run_git(&source, ["config", "user.email", "heimr@example.test"]);
    run_git(&source, ["config", "user.name", "Heimr Test"]);
    fs::write(source.join("README.md"), "first\n").unwrap();
    run_git(&source, ["add", "README.md"]);
    run_git(&source, ["commit", "-m", "first"]);
    let first_commit = git_stdout(&source, ["rev-parse", "HEAD"]);
    fs::write(source.join("README.md"), "second\n").unwrap();
    run_git(&source, ["commit", "-am", "second"]);
    run_git(&source, ["checkout", &first_commit]);

    let workspace = Workspace::open(&temp, "task").unwrap();
    workspace.create().unwrap();
    workspace.prepare_repository(&source).unwrap();

    let repository = workspace.repository_path();
    assert_eq!(
        fs::read_to_string(repository.join("README.md")).unwrap(),
        "first\n"
    );
    fs::remove_dir_all(&source).unwrap();
    assert_self_contained_worktree(&repository);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn prepared_repository_from_a_linked_worktree_source_is_self_contained_after_the_source_is_removed()
{
    let temp = temporary_directory();
    let main = temp.join("main");
    run_git(&temp, ["init", main.to_str().unwrap()]);
    run_git(&main, ["config", "user.email", "heimr@example.test"]);
    run_git(&main, ["config", "user.name", "Heimr Test"]);
    fs::write(main.join("README.md"), "main\n").unwrap();
    run_git(&main, ["add", "README.md"]);
    run_git(&main, ["commit", "-m", "initial"]);
    run_git(&main, ["branch", "linked"]);
    let linked_source = temp.join("linked-source");
    run_git(
        &main,
        ["worktree", "add", linked_source.to_str().unwrap(), "linked"],
    );
    assert!(linked_source.join(".git").is_file());

    let workspace = Workspace::open(&temp, "task").unwrap();
    workspace.create().unwrap();
    workspace.prepare_repository(&linked_source).unwrap();

    let repository = workspace.repository_path();
    assert_eq!(
        fs::read_to_string(repository.join("README.md")).unwrap(),
        "main\n"
    );
    // Removing the whole main repository (and with it the linked worktree's
    // administrative directory under main/.git/worktrees) must not affect
    // the prepared repository at all.
    fs::remove_dir_all(&main).unwrap();
    let _ = fs::remove_dir_all(&linked_source);
    assert_self_contained_worktree(&repository);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn check_detects_a_repository_with_external_git_metadata() {
    let temp = temporary_directory();
    let main = temp.join("main");
    run_git(&temp, ["init", main.to_str().unwrap()]);
    run_git(&main, ["config", "user.email", "heimr@example.test"]);
    run_git(&main, ["config", "user.name", "Heimr Test"]);
    fs::write(main.join("README.md"), "main\n").unwrap();
    run_git(&main, ["add", "README.md"]);
    run_git(&main, ["commit", "-m", "initial"]);

    let workspace = Workspace::open(&temp, "task").unwrap();
    workspace.create().unwrap();
    // Simulate the previous, non-portable behavior directly: a linked
    // worktree placed at the workspace repository path whose Git
    // administrative directory lives outside the workspace entirely.
    run_git(
        &main,
        [
            "worktree",
            "add",
            "--detach",
            workspace.repository_path().to_str().unwrap(),
            "HEAD",
        ],
    );
    let error = workspace.check().unwrap_err();
    assert!(
        error.contains("linked worktree") || error.contains("outside"),
        "{error}"
    );
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn preamble_names_the_fixed_sandbox_context_and_forge_boundary() {
    for kind in ["build", "review"] {
        let output = Command::new(env!("CARGO_BIN_EXE_heimr"))
            .args(["preamble", kind])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let preamble = String::from_utf8(output.stdout).unwrap();
        assert!(preamble.contains("/workspace/WORK.md"));
        assert!(preamble.contains("/workspace/dispatches"));
        assert!(preamble.contains("HANDOFF.json"));
        assert!(preamble.contains("no Skald or Rata"));
        assert!(preamble.contains("gh or az"));
        assert!(preamble.contains("mcp__tools"));
    }
    let build = Command::new(env!("CARGO_BIN_EXE_heimr"))
        .args(["preamble", "build"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8(build.stdout)
            .unwrap()
            .contains("Build the dispatched task")
    );
    let review = Command::new(env!("CARGO_BIN_EXE_heimr"))
        .args(["preamble", "review"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8(review.stdout)
            .unwrap()
            .contains("Review the dispatched task")
    );

    let bogus = Command::new(env!("CARGO_BIN_EXE_heimr"))
        .args(["preamble", "bogus"])
        .output()
        .unwrap();
    assert!(!bogus.status.success());
    assert!(
        String::from_utf8(bogus.stderr)
            .unwrap()
            .contains("kind must be build or review")
    );
}

#[test]
fn template_scaffolds_match_the_documented_schema_per_kind() {
    let build = Command::new(env!("CARGO_BIN_EXE_heimr"))
        .args(["template", "build"])
        .output()
        .unwrap();
    assert!(build.status.success(), "{build:?}");
    let build = String::from_utf8(build.stdout).unwrap();
    for heading in [
        "## Goal",
        "## Acceptance criteria",
        "## Constraints",
        "## Verification",
        "## Deliverable",
    ] {
        assert!(
            build.contains(heading),
            "missing {heading} in build template"
        );
    }

    let review = Command::new(env!("CARGO_BIN_EXE_heimr"))
        .args(["template", "review"])
        .output()
        .unwrap();
    assert!(review.status.success(), "{review:?}");
    let review = String::from_utf8(review.stdout).unwrap();
    for heading in [
        "## Frame",
        "## Acceptance criteria",
        "## Review checklist",
        "## Read-only boundary",
        "## Expected HANDOFF.json shape",
    ] {
        assert!(
            review.contains(heading),
            "missing {heading} in review template"
        );
    }

    let bogus = Command::new(env!("CARGO_BIN_EXE_heimr"))
        .args(["template", "bogus"])
        .output()
        .unwrap();
    assert!(!bogus.status.success());
    assert!(
        String::from_utf8(bogus.stderr)
            .unwrap()
            .contains("kind must be build or review")
    );
}

#[test]
fn preamble_and_template_do_not_require_a_workspace_root() {
    for command in [["preamble", "build"], ["template", "review"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_heimr"))
            .args(command)
            .env_remove("HEIMR_ROOT")
            .env_remove("HOME")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
}

#[test]
fn help_and_docs_do_not_require_a_workspace_root() {
    let help = Command::new(env!("CARGO_BIN_EXE_heimr"))
        .arg("help")
        .output()
        .unwrap();
    assert!(help.status.success(), "{help:?}");
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("Commands:"));
    assert!(help.contains("docs"));

    let docs = Command::new(env!("CARGO_BIN_EXE_heimr"))
        .arg("docs")
        .output()
        .unwrap();
    assert!(docs.status.success(), "{docs:?}");
    let docs = String::from_utf8(docs.stdout).unwrap();
    assert!(docs.starts_with("# Agent Usage"));
    assert!(docs.contains("heimr check <workspace>"));
    assert!(docs.contains("heimr preamble build"));
    assert!(docs.contains("heimr template build"));
}
