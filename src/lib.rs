use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

static TEMPORARY_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug)]
pub struct Workspace {
    pub root: PathBuf,
}

impl Workspace {
    pub fn open(root: &Path, name: &str) -> Result<Self> {
        validate_name("workspace name", name)?;
        Ok(Self {
            root: root.join(name),
        })
    }

    pub fn create(&self) -> Result<()> {
        if self.root.exists() {
            return Err(format!("workspace already exists: {}", self.root.display()));
        }
        fs::create_dir_all(self.root.join("dispatches")).map_err(io_error)
    }

    pub fn work_path(&self) -> PathBuf {
        self.root.join("WORK.md")
    }
    pub fn repository_path(&self) -> PathBuf {
        self.root.join("repository")
    }
    pub fn dispatches_path(&self) -> PathBuf {
        self.root.join("dispatches")
    }
    pub fn dispatch_path(&self, name: &str) -> Result<PathBuf> {
        validate_name("dispatch name", name)?;
        Ok(self.dispatches_path().join(name))
    }

    pub fn set_work(&self, content: &[u8]) -> Result<()> {
        self.require_exists()?;
        write_new(&self.work_path(), content)
    }

    /// Prepares the workspace repository from an existing local checkout.
    ///
    /// The prepared repository is always a fully self-contained clone: it
    /// never reuses the source checkout's Git administrative directory, so
    /// the result works identically whether `source` is a branch tip, a
    /// detached commit, or itself a linked worktree.
    pub fn prepare_repository(&self, source: &Path) -> Result<()> {
        self.require_exists()?;
        if self.repository_path().exists() {
            return validate_worktree(&self.repository_path());
        }
        let source = source.canonicalize().map_err(io_error)?;
        let commit = git_stdout(
            Command::new("git")
                .arg("-C")
                .arg(&source)
                .args(["rev-parse", "HEAD"]),
            "failed to resolve source HEAD",
        )?;
        self.install_clone(|clone| {
            run_git(
                Command::new("git")
                    .args(["clone", "--no-hardlinks"])
                    .arg(&source)
                    .arg(clone),
                "git clone failed",
            )?;
            run_git(
                Command::new("git")
                    .arg("-C")
                    .arg(clone)
                    .args(["switch", "--detach"])
                    .arg(&commit),
                "git checkout failed",
            )?;
            detach_from_clone_origin(clone)
        })
    }

    pub fn prepare_repository_url(&self, url: &str) -> Result<()> {
        self.require_exists()?;
        if self.repository_path().exists() {
            return validate_worktree(&self.repository_path());
        }
        self.install_clone(|clone| {
            run_git(
                Command::new("git").args(["clone", url]).arg(clone),
                "git clone failed",
            )?;
            run_git(
                Command::new("git")
                    .arg("-C")
                    .arg(clone)
                    .args(["switch", "--detach"]),
                "git checkout failed",
            )?;
            detach_from_clone_origin(clone)
        })
    }

    /// Stages a repository built by `populate` under a temporary directory
    /// inside the workspace, validates it is a self-contained worktree, and
    /// atomically installs it as the workspace repository. The staging
    /// directory is always removed, whether or not `populate` succeeds.
    fn install_clone(&self, populate: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
        let staging = self.create_clone_staging_directory()?;
        let clone = staging.join("repository");
        let result = (|| {
            populate(&clone)?;
            validate_worktree(&clone)?;
            fs::rename(&clone, self.repository_path()).map_err(io_error)
        })();
        let cleanup = fs::remove_dir_all(&staging).map_err(io_error);
        result?;
        cleanup
    }

    pub fn new_dispatch(&self, name: &str) -> Result<()> {
        self.require_exists()?;
        let path = self.dispatch_path(name)?;
        if path.exists() {
            return Err(format!("dispatch already exists: {name}"));
        }
        fs::create_dir(path).map_err(io_error)
    }

    pub fn put_dispatch_file(
        &self,
        dispatch: &str,
        relative_path: &Path,
        content: &[u8],
    ) -> Result<()> {
        let directory = self.require_unsealed_dispatch(dispatch)?;
        let relative_path = validate_relative_path(relative_path)?;
        if relative_path == Path::new("dispatch.json") || relative_path == Path::new("HANDOFF.json")
        {
            return Err(
                "dispatch.json and HANDOFF.json are reserved Heimr metadata paths".to_owned(),
            );
        }
        let destination = directory.join(relative_path);
        ensure_confined_parent(&directory, relative_path)?;
        write_new(&destination, content)
    }

    pub fn seal_dispatch(&self, dispatch: &str) -> Result<PathBuf> {
        let directory = self.require_unsealed_dispatch(dispatch)?;
        let handoff = directory.join("HANDOFF.json");
        match fs::symlink_metadata(&handoff) {
            Ok(metadata) if metadata.file_type().is_file() => {}
            Ok(_) => return Err("HANDOFF.json must be a regular file".to_owned()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => write_new(
                &handoff,
                b"{\n  \"version\": 1,\n  \"status\": \"pending\"\n}\n",
            )?,
            Err(error) => return Err(io_error(error)),
        }
        let inventory = inventory(&directory)?;
        let record = format!(
            "{{\n  \"version\": 1,\n  \"files\": [\n{}\n  ]\n}}\n",
            inventory
                .iter()
                .map(|entry| format!(
                    "    {{\"path\": \"{}\", \"sha256\": \"{}\"}}",
                    json_escape(&entry.path),
                    entry.digest
                ))
                .collect::<Vec<_>>()
                .join(",\n")
        );
        let record_path = directory.join("dispatch.json");
        write_new(&record_path, record.as_bytes())?;
        Ok(record_path)
    }

    pub fn handoff(&self, dispatch: &str) -> Result<Vec<u8>> {
        self.require_exists()?;
        let directory = self.require_sealed_dispatch(dispatch)?;
        verify_dispatch(&directory, &directory.join("dispatch.json"))?;
        fs::read(directory.join("HANDOFF.json")).map_err(io_error)
    }

    pub fn check(&self) -> Result<()> {
        self.require_exists()?;
        if !self.dispatches_path().is_dir() {
            return Err("workspace is missing dispatches/".to_owned());
        }
        if self.repository_path().exists() {
            validate_worktree(&self.repository_path())?;
        }
        for entry in fs::read_dir(self.dispatches_path()).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            if !entry.file_type().map_err(io_error)?.is_dir() {
                return Err("dispatches contains a non-directory entry".to_owned());
            }
            let record = entry.path().join("dispatch.json");
            if record.exists() {
                verify_dispatch(&entry.path(), &record)?;
            }
        }
        Ok(())
    }

    fn require_exists(&self) -> Result<()> {
        if self.root.is_dir() {
            Ok(())
        } else {
            Err(format!("workspace does not exist: {}", self.root.display()))
        }
    }

    fn create_clone_staging_directory(&self) -> Result<PathBuf> {
        for _ in 0..100 {
            let sequence = TEMPORARY_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = self.root.join(format!(
                ".repository-clone-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io_error(error)),
            }
        }
        Err("could not create a unique repository clone staging directory".to_owned())
    }
    fn require_unsealed_dispatch(&self, name: &str) -> Result<PathBuf> {
        let path = self.dispatch_path(name)?;
        if !path.is_dir() {
            return Err(format!("dispatch does not exist: {name}"));
        }
        if path.join("dispatch.json").exists() {
            return Err(format!("dispatch is sealed: {name}"));
        }
        Ok(path)
    }

    fn require_sealed_dispatch(&self, name: &str) -> Result<PathBuf> {
        let path = self.dispatch_path(name)?;
        if !path.is_dir() {
            return Err(format!("dispatch does not exist: {name}"));
        }
        if !path.join("dispatch.json").is_file() {
            return Err(format!("dispatch is not sealed: {name}"));
        }
        Ok(path)
    }
}

#[derive(Debug)]
struct InventoryEntry {
    path: String,
    digest: String,
}

fn inventory(directory: &Path) -> Result<Vec<InventoryEntry>> {
    let mut files = Vec::new();
    collect_files(directory, directory, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn collect_files(base: &Path, directory: &Path, files: &mut Vec<InventoryEntry>) -> Result<()> {
    for entry in fs::read_dir(directory).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        let kind = entry.file_type().map_err(io_error)?;
        if kind.is_dir() {
            collect_files(base, &path, files)?;
        } else if kind.is_file() {
            let relative = path.strip_prefix(base).map_err(|error| error.to_string())?;
            if relative != Path::new("dispatch.json") && relative != Path::new("HANDOFF.json") {
                files.push(InventoryEntry {
                    path: path_to_slashes(relative)?,
                    digest: sha256_file(&path)?,
                });
            }
        } else if !kind.is_file() {
            return Err(format!(
                "dispatch contains unsupported entry: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn verify_dispatch(directory: &Path, record: &Path) -> Result<()> {
    let handoff = directory.join("HANDOFF.json");
    match fs::symlink_metadata(&handoff) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => return Err("HANDOFF.json must be a regular file".to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err("sealed dispatch is missing HANDOFF.json".to_owned());
        }
        Err(error) => return Err(io_error(error)),
    }

    let actual = fs::read_to_string(record).map_err(io_error)?;
    let entries = inventory(directory)?;
    let expected = format!(
        "{{\n  \"version\": 1,\n  \"files\": [\n{}\n  ]\n}}\n",
        entries
            .iter()
            .map(|entry| format!(
                "    {{\"path\": \"{}\", \"sha256\": \"{}\"}}",
                json_escape(&entry.path),
                entry.digest
            ))
            .collect::<Vec<_>>()
            .join(",\n")
    );
    if actual == expected || legacy_inventory_matches_without_handoff(&actual, &expected) {
        Ok(())
    } else {
        Err(format!(
            "sealed dispatch inventory does not match: {}",
            directory.display()
        ))
    }
}

/// Accept inventories written before `HANDOFF.json` became mutable. The old
/// handoff digest is intentionally discarded; every remaining entry must
/// still exactly match the current immutable inventory.
fn legacy_inventory_matches_without_handoff(actual: &str, expected: &str) -> bool {
    const HEADER: &str = "{\n  \"version\": 1,\n  \"files\": [\n";
    const FOOTER: &str = "\n  ]\n}\n";
    const HANDOFF_PREFIX: &str = "    {\"path\": \"HANDOFF.json\", \"sha256\": \"";

    let Some(actual_files) = actual
        .strip_prefix(HEADER)
        .and_then(|value| value.strip_suffix(FOOTER))
    else {
        return false;
    };
    let Some(expected_files) = expected
        .strip_prefix(HEADER)
        .and_then(|value| value.strip_suffix(FOOTER))
    else {
        return false;
    };

    let mut found_handoff = false;
    let immutable_files = actual_files
        .lines()
        .filter(|line| {
            if line.starts_with(HANDOFF_PREFIX) {
                found_handoff = true;
                false
            } else {
                true
            }
        })
        .map(|line| line.strip_suffix(',').unwrap_or(line))
        .collect::<Vec<_>>();

    found_handoff && immutable_files.join(",\n") == expected_files
}

/// Confirms `path` is a Git worktree whose administrative directory,
/// common directory, and object alternates all live under `path` itself.
/// This is what makes ordinary Git history and diff operations work
/// without touching any path outside the prepared repository: a linked
/// worktree (whose `.git` file points at a separate repository's
/// `.git/worktrees/<name>` directory) or an alternates file referencing
/// an external object store fails this check.
fn validate_worktree(path: &Path) -> Result<()> {
    let canonical = path.canonicalize().map_err(io_error)?;

    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map_err(io_error)?;
    if !(output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true") {
        return Err(format!(
            "repository is not a valid Git worktree: {}",
            path.display()
        ));
    }

    let git_dir_raw = git_stdout(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "--absolute-git-dir"]),
        "failed to resolve the repository's Git directory",
    )?;
    let git_dir = PathBuf::from(&git_dir_raw)
        .canonicalize()
        .map_err(io_error)?;
    if !git_dir.starts_with(&canonical) {
        return Err(format!(
            "repository Git metadata lives outside the prepared repository: {}",
            git_dir.display()
        ));
    }

    let common_dir_raw = git_stdout(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "--git-common-dir"]),
        "failed to resolve the repository's common Git directory",
    )?;
    let common_dir = PathBuf::from(&common_dir_raw);
    let common_dir = if common_dir.is_absolute() {
        common_dir
    } else {
        path.join(common_dir)
    };
    let common_dir = common_dir.canonicalize().map_err(io_error)?;
    if common_dir != git_dir {
        return Err(format!(
            "repository is a linked worktree referencing external Git metadata: {}",
            common_dir.display()
        ));
    }

    let alternates = git_dir.join("objects/info/alternates");
    if alternates.exists() {
        let content = fs::read_to_string(&alternates).map_err(io_error)?;
        for line in content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            let alternate = PathBuf::from(line);
            let alternate = if alternate.is_absolute() {
                alternate
            } else {
                git_dir.join("objects").join(alternate)
            };
            let alternate = alternate.canonicalize().map_err(io_error)?;
            if !alternate.starts_with(&canonical) {
                return Err(format!(
                    "repository objects depend on an external alternate store: {}",
                    alternate.display()
                ));
            }
        }
    }

    Ok(())
}

/// Removes the `origin` remote left behind by a local clone so the prepared
/// repository does not retain Git configuration pointing at the source
/// checkout's filesystem path.
fn detach_from_clone_origin(path: &Path) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["remote", "remove", "origin"])
        .output()
        .map_err(io_error)?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if detail.contains("No such remote") {
        Ok(())
    } else {
        Err(format!("failed to remove clone origin remote: {detail}"))
    }
}

fn run_git(command: &mut Command, context: &str) -> Result<()> {
    let output = command.output().map_err(io_error)?;
    if output.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if detail.is_empty() {
            Err(context.to_owned())
        } else {
            Err(format!("{context}: {detail}"))
        }
    }
}

fn git_stdout(command: &mut Command, context: &str) -> Result<String> {
    let output = command.output().map_err(io_error)?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if detail.is_empty() {
            Err(context.to_owned())
        } else {
            Err(format!("{context}: {detail}"))
        }
    }
}

pub fn list_workspaces(root: &Path) -> Result<Vec<String>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(root).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        if entry.file_type().map_err(io_error)?.is_dir() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

pub fn read_input(from: Option<&Path>) -> Result<Vec<u8>> {
    match from {
        Some(path) => fs::read(path).map_err(io_error),
        None => {
            let mut input = Vec::new();
            io::stdin().read_to_end(&mut input).map_err(io_error)?;
            Ok(input)
        }
    }
}

pub fn validate_name(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        Err(format!("{label} must be a single non-empty path component"))
    } else {
        Ok(())
    }
}

pub fn validate_relative_path(path: &Path) -> Result<&Path> {
    if path.is_absolute()
        || path.as_os_str().is_empty()
        || path.components().any(|part| !matches!(part, Component::Normal(name) if !name.to_string_lossy().chars().any(char::is_control)))
    {
        Err("dispatch path must be a confined relative path".to_owned())
    } else {
        Ok(path)
    }
}

fn ensure_confined_parent(directory: &Path, relative_path: &Path) -> Result<()> {
    let mut current = directory.to_path_buf();
    let components = relative_path.components().collect::<Vec<_>>();
    for component in &components[..components.len().saturating_sub(1)] {
        let Component::Normal(name) = component else {
            return Err("dispatch path must be a confined relative path".to_owned());
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "dispatch path contains a symlink: {}",
                    current.display()
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(format!(
                    "dispatch path component is not a directory: {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(io_error)?
            }
            Err(error) => return Err(io_error(error)),
        }
    }
    if let Ok(metadata) = fs::symlink_metadata(directory.join(relative_path))
        && metadata.file_type().is_symlink()
    {
        return Err("dispatch path is a symlink".to_owned());
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).map_err(io_error)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let count = file.read(&mut buffer).map_err(io_error)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn path_to_slashes(path: &Path) -> Result<String> {
    path.to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| "dispatch path is not valid UTF-8".to_owned())
}
fn json_escape(value: &str) -> String {
    use std::fmt::Write as _;
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            character if character.is_control() => write!(escaped, "\\u{:04x}", character as u32)
                .expect("writing to String cannot fail"),
            character => escaped.push(character),
        }
    }
    escaped
}
fn io_error(error: io::Error) -> String {
    error.to_string()
}

fn write_new(path: &Path, content: &[u8]) -> Result<()> {
    if path.exists() {
        return Err(format!("refusing to overwrite {}", path.display()));
    }
    let temporary = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        TEMPORARY_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(io_error)?;
        file.write_all(content).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        if path.exists() {
            return Err(format!("refusing to overwrite {}", path.display()));
        }
        fs::rename(&temporary, path).map_err(io_error)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
