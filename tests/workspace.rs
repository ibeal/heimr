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
    assert!(
        workspace
            .check()
            .unwrap_err()
            .contains("inventory does not match")
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
    assert!(!altered.status.success());
    assert!(
        String::from_utf8(altered.stderr)
            .unwrap()
            .contains("inventory does not match")
    );

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

fn run_git<const N: usize>(directory: &std::path::Path, args: [&str; N]) {
    let status = Command::new("git")
        .current_dir(directory)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success());
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
}
