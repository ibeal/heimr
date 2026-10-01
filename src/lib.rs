use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

static TEMPORARY_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug)]
pub struct Workspace {
    pub root: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase", tag = "kind")]
pub enum WorkspaceSource {
    Mounted {
        path: String,
        revision: Option<String>,
    },
    Cloned {
        identity: String,
        revision: String,
    },
}

#[derive(Debug, Deserialize, Serialize)]
struct WorkspaceRecord {
    version: u32,
    #[serde(default)]
    legacy: bool,
    #[serde(default)]
    template: Option<TemplateSelection>,
    source: Option<WorkspaceSource>,
    environment: Vec<InventoryEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TemplateSelection {
    name: String,
    environment: Vec<InventoryEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct EnvironmentManifest {
    version: u32,
    files: Vec<InventoryEntry>,
}

#[derive(Debug, Serialize)]
pub struct MountPlan {
    pub version: u32,
    pub mounts: Vec<Mount>,
}

#[derive(Debug, Serialize)]
pub struct Mount {
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

const ENVIRONMENT_FILES: [&str; 4] = ["AGENTS.md", "task.md", "prompt.md", "context/INDEX.md"];
pub const DEFAULT_TEMPLATE: &str = "default";

#[derive(Clone, Debug)]
pub struct TemplateWorkspace {
    pub root: PathBuf,
    name: String,
}

impl TemplateWorkspace {
    pub fn open(heimr_root: &Path, name: &str) -> Result<Self> {
        validate_name("template name", name)?;
        Ok(Self {
            root: heimr_root.join(".templates").join(name),
            name: name.to_owned(),
        })
    }

    pub fn create(&self) -> Result<()> {
        if self.root.exists() {
            return Err(format!(
                "template workspace already exists: {}",
                self.root.display()
            ));
        }
        fs::create_dir_all(self.environment_path().join("context")).map_err(io_error)?;
        for relative in ENVIRONMENT_FILES {
            write_new(&self.environment_path().join(relative), b"")?;
        }
        self.refresh_manifest()
    }

    pub fn put_environment_file(&self, relative_path: &Path, content: &[u8]) -> Result<()> {
        self.require_exists()?;
        let relative_path = validate_environment_path(relative_path)?;
        ensure_confined_parent(&self.environment_path(), relative_path)?;
        write_replacing(&self.environment_path().join(relative_path), content)?;
        self.refresh_manifest()
    }

    pub fn check(&self) -> Result<()> {
        self.require_exists()?;
        validate_curated_environment(&self.environment_path())?;
        Ok(())
    }

    fn environment_path(&self) -> PathBuf {
        self.root.join("environment")
    }

    fn require_exists(&self) -> Result<()> {
        if self.root.is_dir() {
            Ok(())
        } else {
            Err(format!(
                "template workspace does not exist: {}",
                self.root.display()
            ))
        }
    }

    fn refresh_manifest(&self) -> Result<()> {
        let files = environment_inventory(&self.environment_path())?;
        write_environment_manifest(&self.environment_path(), &files)
    }
}

impl Workspace {
    pub fn open(root: &Path, name: &str) -> Result<Self> {
        validate_name("workspace name", name)?;
        if name == ".templates" {
            return Err("workspace name is reserved for Heimr template storage".to_owned());
        }
        Ok(Self {
            root: root.join(name),
        })
    }

    pub fn create(&self) -> Result<()> {
        self.create_from_template(DEFAULT_TEMPLATE)
    }

    pub fn create_from_template(&self, template_name: &str) -> Result<()> {
        if self.root.exists() {
            return Err(format!("workspace already exists: {}", self.root.display()));
        }
        let heimr_root = self
            .root
            .parent()
            .ok_or_else(|| "workspace has no Heimr root".to_owned())?;
        let template = TemplateWorkspace::open(heimr_root, template_name)?;
        if template_name == DEFAULT_TEMPLATE && !template.root.exists() {
            template.create()?;
        }
        template.check()?;
        let template_files = environment_inventory(&template.environment_path())?;
        fs::create_dir_all(self.root.join("dispatches")).map_err(io_error)?;
        let result = self.install_environment_template(
            None,
            false,
            Some(TemplateSelection {
                name: template.name.clone(),
                environment: template_files,
            }),
            Some(&template.environment_path()),
        );
        match result {
            Ok(()) => Ok(()),
            Err(error) if !self.root.exists() => Err(error),
            Err(error) => match fs::remove_dir_all(&self.root) {
                Ok(()) => Err(error),
                Err(cleanup) => Err(format!(
                    "{error}; additionally failed to remove partial workspace: {cleanup}"
                )),
            },
        }
    }

    pub fn migrate(&self) -> Result<()> {
        self.require_exists()?;
        if self.workspace_record_path().exists() {
            return self.validate_environment_template();
        }
        if self.environment_path().exists() || self.state_path().exists() {
            return Err("legacy workspace has a partial curated template; remove or repair it before migration".to_owned());
        }
        let source = if self.repository_path().exists() {
            validate_worktree(&self.repository_path())?;
            Some(WorkspaceSource::Cloned {
                identity: "unknown (migrated legacy repository)".to_owned(),
                revision: repository_revision(&self.repository_path())?,
            })
        } else {
            None
        };
        self.install_environment_template(source, true, None, None)
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
    pub fn environment_path(&self) -> PathBuf {
        self.root.join("environment")
    }
    pub fn state_path(&self) -> PathBuf {
        self.root.join("state")
    }
    pub fn workspace_record_path(&self) -> PathBuf {
        self.root.join("workspace.json")
    }
    pub fn dispatch_path(&self, name: &str) -> Result<PathBuf> {
        validate_name("dispatch name", name)?;
        Ok(self.dispatches_path().join(name))
    }

    pub fn set_work(&self, content: &[u8]) -> Result<()> {
        self.require_exists()?;
        write_replacing(&self.work_path(), content)
    }

    pub fn set_mounted_source(&self, source: &Path) -> Result<()> {
        self.require_new_style()?;
        if !source.is_dir() {
            return Err(format!(
                "mounted source is not a directory: {}",
                source.display()
            ));
        }
        let source = source.canonicalize().map_err(io_error)?;
        reject_heimr_root_overlap(&self.canonical_heimr_root()?, &source)?;
        let source_path = path_string(&source)?;
        match self.read_workspace_record()?.source {
            None => self.update_source(Some(WorkspaceSource::Mounted {
                path: source_path,
                revision: try_repository_revision(&source),
            })),
            Some(WorkspaceSource::Mounted { path, .. }) if path == source_path => Ok(()),
            Some(_) => Err(
                "workspace source is already selected; recreate the workspace to select a different source"
                    .to_owned(),
            ),
        }
    }

    pub fn put_environment_file(&self, relative_path: &Path, content: &[u8]) -> Result<()> {
        self.require_new_style()?;
        let relative_path = validate_environment_path(relative_path)?;
        let destination = self.environment_path().join(relative_path);
        ensure_confined_parent(&self.environment_path(), relative_path)?;
        write_replacing(&destination, content)?;
        self.refresh_environment_metadata()
    }

    pub fn mount_plan(&self) -> Result<MountPlan> {
        self.require_new_style()?;
        self.validate_environment_template()?;
        let record = self.read_workspace_record()?;
        let repository = match record.source.ok_or_else(|| {
            "workspace source is not selected; use `source mount` or `repo prepare`".to_owned()
        })? {
            WorkspaceSource::Mounted { path, .. } => {
                let path = PathBuf::from(path);
                if !path.is_dir() {
                    return Err(format!(
                        "mounted source is not a directory: {}",
                        path.display()
                    ));
                }
                let path = path.canonicalize().map_err(io_error)?;
                reject_heimr_root_overlap(&self.canonical_heimr_root()?, &path)?;
                path
            }
            WorkspaceSource::Cloned { .. } => {
                if !self.repository_path().is_dir() {
                    return Err(format!(
                        "workspace repository does not exist: {}",
                        self.repository_path().display()
                    ));
                }
                validate_worktree(&self.repository_path())?;
                self.repository_path().canonicalize().map_err(io_error)?
            }
        };
        let root = self.root.canonicalize().map_err(io_error)?;
        let environment = confined_directory(&root, &self.environment_path(), "environment")?;
        let state = confined_directory(&root, &self.state_path(), "state")?;
        Ok(MountPlan {
            version: 1,
            mounts: vec![
                Mount {
                    source: path_string(&repository)?,
                    target: "/repo".to_owned(),
                    read_only: false,
                },
                Mount {
                    source: path_string(&environment)?,
                    target: "/agent".to_owned(),
                    read_only: true,
                },
                Mount {
                    source: path_string(&state)?,
                    target: "/agent-state".to_owned(),
                    read_only: false,
                },
            ],
        })
    }

    /// Prepares the workspace repository from an existing local checkout.
    ///
    /// The prepared repository is always a fully self-contained clone: it
    /// never reuses the source checkout's Git administrative directory, so
    /// the result works identically whether `source` is a branch tip, a
    /// detached commit, or itself a linked worktree.
    pub fn prepare_repository(&self, source: &Path) -> Result<()> {
        self.require_exists()?;
        let source = source.canonicalize().map_err(io_error)?;
        let commit = git_stdout(
            Command::new("git")
                .arg("-C")
                .arg(&source)
                .args(["rev-parse", "HEAD"]),
            "failed to resolve source HEAD",
        )?;
        if self.repository_path().exists() {
            validate_worktree(&self.repository_path())?;
            return self.validate_existing_clone_identity(&path_string(&source)?);
        }
        self.require_source_unselected()?;
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
        })?;
        if self.workspace_record_path().exists() {
            self.update_source(Some(WorkspaceSource::Cloned {
                identity: path_string(&source)?,
                revision: commit,
            }))?;
        }
        Ok(())
    }

    pub fn prepare_repository_url(&self, url: &str) -> Result<()> {
        self.require_exists()?;
        if self.repository_path().exists() {
            validate_worktree(&self.repository_path())?;
            return self.validate_existing_clone_identity(url);
        }
        self.require_source_unselected()?;
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
        })?;
        if self.workspace_record_path().exists() {
            self.update_source(Some(WorkspaceSource::Cloned {
                identity: url.to_owned(),
                revision: repository_revision(&self.repository_path())?,
            }))?;
        }
        Ok(())
    }

    /// Sets (or updates) the workspace repository's `origin` push remote to
    /// an opaque URL supplied by the caller. Heimr never constructs,
    /// guesses, or interprets this URL: `prepare_repository`/
    /// `prepare_repository_url` strip any clone-origin remote so the
    /// prepared worktree is self-contained, and this is the only supported
    /// way to (re)attach a remote for a later push. Calling it again with a
    /// different URL updates the existing remote rather than failing.
    pub fn set_push_remote(&self, url: &str) -> Result<()> {
        self.require_exists()?;
        let repository = self.repository_path();
        if !repository.is_dir() {
            return Err(format!(
                "workspace repository does not exist: {} (run `repo prepare` first)",
                repository.display()
            ));
        }
        set_remote(&repository, "origin", url)
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
                    entry.sha256
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
        if self.workspace_record_path().exists() {
            self.validate_environment_template()?;
            let record = self.read_workspace_record()?;
            match record.source {
                Some(WorkspaceSource::Mounted { path, .. }) => {
                    let path = PathBuf::from(path);
                    if !path.is_dir() {
                        return Err(format!(
                            "mounted source is not a directory: {}",
                            path.display()
                        ));
                    }
                    let path = path.canonicalize().map_err(io_error)?;
                    reject_heimr_root_overlap(&self.canonical_heimr_root()?, &path)?;
                }
                Some(WorkspaceSource::Cloned { .. }) => {
                    if !self.repository_path().is_dir() {
                        return Err(format!(
                            "workspace repository does not exist: {}",
                            self.repository_path().display()
                        ));
                    }
                    validate_worktree(&self.repository_path())?;
                }
                None if record.legacy => {}
                None => {
                    return Err(
                        "workspace source is not selected; use `source mount` or `repo prepare`"
                            .to_owned(),
                    );
                }
            }
        }
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

    fn canonical_heimr_root(&self) -> Result<PathBuf> {
        self.root
            .parent()
            .ok_or_else(|| "workspace has no Heimr root".to_owned())?
            .canonicalize()
            .map_err(io_error)
    }

    fn require_exists(&self) -> Result<()> {
        if self.root.is_dir() {
            Ok(())
        } else {
            Err(format!("workspace does not exist: {}", self.root.display()))
        }
    }

    fn require_new_style(&self) -> Result<()> {
        self.require_exists()?;
        if self.workspace_record_path().is_file() {
            Ok(())
        } else {
            Err(
                "legacy workspace has no curated environment; run `heimr migrate <workspace>`"
                    .to_owned(),
            )
        }
    }

    fn install_environment_template(
        &self,
        source: Option<WorkspaceSource>,
        legacy: bool,
        template: Option<TemplateSelection>,
        template_environment: Option<&Path>,
    ) -> Result<()> {
        if !self.work_path().exists() {
            write_new(&self.work_path(), b"")?;
        }
        if let Some(template_environment) = template_environment {
            copy_curated_environment(template_environment, &self.environment_path())?;
        } else {
            fs::create_dir_all(self.environment_path().join("context")).map_err(io_error)?;
            for relative in ENVIRONMENT_FILES {
                let path = self.environment_path().join(relative);
                if !path.exists() {
                    write_new(&path, b"")?;
                }
            }
        }
        fs::create_dir_all(self.state_path()).map_err(io_error)?;
        let handoff = self.state_path().join("HANDOFF.json");
        if !handoff.exists() {
            write_new(
                &handoff,
                b"{\n  \"version\": 1,\n  \"status\": \"pending\"\n}\n",
            )?;
        }
        let files = environment_inventory(&self.environment_path())?;
        self.write_environment_manifest(&files)?;
        self.write_workspace_record(&WorkspaceRecord {
            version: 1,
            legacy,
            template,
            source,
            environment: files,
        })
    }

    fn refresh_environment_metadata(&self) -> Result<()> {
        let files = environment_inventory(&self.environment_path())?;
        self.write_environment_manifest(&files)?;
        let mut record = self.read_workspace_record()?;
        record.environment = files;
        self.write_workspace_record(&record)
    }

    fn update_source(&self, source: Option<WorkspaceSource>) -> Result<()> {
        let mut record = self.read_workspace_record()?;
        record.source = source;
        self.write_workspace_record(&record)
    }

    fn require_source_unselected(&self) -> Result<()> {
        if self.workspace_record_path().exists() && self.read_workspace_record()?.source.is_some() {
            Err(
                "workspace source is already selected; recreate the workspace to select a different source"
                    .to_owned(),
            )
        } else {
            Ok(())
        }
    }

    fn validate_existing_clone_identity(&self, requested: &str) -> Result<()> {
        if !self.workspace_record_path().exists() {
            return Ok(());
        }
        match self.read_workspace_record()?.source {
            Some(WorkspaceSource::Cloned { identity, .. }) if identity == requested => Ok(()),
            Some(WorkspaceSource::Cloned { identity, .. }) => Err(format!(
                "workspace repository was prepared from {identity}; refusing to relabel it as {requested}"
            )),
            _ => Err(
                "workspace repository exists but is not the selected source; recreate the workspace to select that clone"
                    .to_owned(),
            ),
        }
    }

    fn read_workspace_record(&self) -> Result<WorkspaceRecord> {
        let content = fs::read(self.workspace_record_path()).map_err(io_error)?;
        serde_json::from_slice(&content).map_err(|error| format!("invalid workspace.json: {error}"))
    }

    fn write_workspace_record(&self, record: &WorkspaceRecord) -> Result<()> {
        let content = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
        write_replacing(
            &self.workspace_record_path(),
            &[content, b"\n".to_vec()].concat(),
        )
    }

    fn write_environment_manifest(&self, files: &[InventoryEntry]) -> Result<()> {
        write_environment_manifest(&self.environment_path(), files)
    }

    fn validate_environment_template(&self) -> Result<()> {
        let environment = self.environment_path();
        let state = self.state_path();
        let root = self.root.canonicalize().map_err(io_error)?;
        for directory in [&environment, &environment.join("context"), &state] {
            if !directory.is_dir() {
                return Err(format!(
                    "invalid workspace template: missing {}/",
                    directory.display()
                ));
            }
        }
        confined_directory(&root, &environment, "environment")?;
        confined_directory(&root, &state, "state")?;
        if !self.workspace_record_path().exists() {
            return Err("invalid workspace template: missing workspace.json".to_owned());
        }
        if !self.workspace_record_path().is_file()
            || fs::symlink_metadata(self.workspace_record_path())
                .map_err(io_error)?
                .file_type()
                .is_symlink()
        {
            return Err(
                "invalid workspace template: workspace.json must be a regular file".to_owned(),
            );
        }
        for relative in ENVIRONMENT_FILES {
            if !environment.join(relative).is_file() {
                return Err(format!(
                    "invalid workspace template: missing environment/{relative}"
                ));
            }
        }
        if !state.join("HANDOFF.json").is_file() {
            return Err("invalid workspace template: missing state/HANDOFF.json".to_owned());
        }
        let files = environment_inventory(&environment)?;
        let manifest: EnvironmentManifest =
            serde_json::from_slice(&fs::read(environment.join("manifest.json")).map_err(io_error)?)
                .map_err(|error| format!("invalid environment/manifest.json: {error}"))?;
        let record = self.read_workspace_record()?;
        if manifest.version != 1
            || record.version != 1
            || manifest.files != files
            || record.environment != files
        {
            return Err("curated environment inventory does not match its metadata".to_owned());
        }
        match &record.template {
            Some(template) => {
                validate_name("template name", &template.name)?;
                validate_inventory_entries(&template.environment)?;
            }
            None if record.legacy => {}
            None => return Err("workspace metadata is missing template provenance".to_owned()),
        }
        Ok(())
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct InventoryEntry {
    path: String,
    sha256: String,
}

fn environment_inventory(directory: &Path) -> Result<Vec<InventoryEntry>> {
    let mut files = Vec::new();
    collect_files(directory, directory, &mut files)?;
    files.retain(|entry| entry.path != "manifest.json");
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn validate_inventory_entries(files: &[InventoryEntry]) -> Result<()> {
    let mut previous: Option<&str> = None;
    for entry in files {
        validate_environment_path(Path::new(&entry.path))?;
        if previous.is_some_and(|value| value >= entry.path.as_str())
            || entry.sha256.len() != 64
            || !entry.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(
                "template provenance contains an invalid curated-input inventory".to_owned(),
            );
        }
        previous = Some(&entry.path);
    }
    Ok(())
}

fn write_environment_manifest(directory: &Path, files: &[InventoryEntry]) -> Result<()> {
    let content = serde_json::to_vec_pretty(&EnvironmentManifest {
        version: 1,
        files: files.to_vec(),
    })
    .map_err(|error| error.to_string())?;
    write_replacing(
        &directory.join("manifest.json"),
        &[content, b"\n".to_vec()].concat(),
    )
}

fn validate_curated_environment(directory: &Path) -> Result<Vec<InventoryEntry>> {
    if !directory.is_dir() || !directory.join("context").is_dir() {
        return Err(format!(
            "invalid template workspace: missing curated environment directories under {}",
            directory.display()
        ));
    }
    for relative in ENVIRONMENT_FILES {
        if !directory.join(relative).is_file() {
            return Err(format!(
                "invalid template workspace: missing environment/{relative}"
            ));
        }
    }
    let files = environment_inventory(directory)?;
    let manifest: EnvironmentManifest =
        serde_json::from_slice(&fs::read(directory.join("manifest.json")).map_err(io_error)?)
            .map_err(|error| format!("invalid template environment/manifest.json: {error}"))?;
    if manifest.version != 1 || manifest.files != files {
        return Err(
            "template curated environment inventory does not match its manifest".to_owned(),
        );
    }
    Ok(files)
}

fn copy_curated_environment(source: &Path, destination: &Path) -> Result<()> {
    let files = validate_curated_environment(source)?;
    fs::create_dir_all(destination.join("context")).map_err(io_error)?;
    for entry in &files {
        let relative = Path::new(&entry.path);
        validate_environment_path(relative)?;
        ensure_confined_parent(destination, relative)?;
        write_new(
            &destination.join(relative),
            &fs::read(source.join(relative)).map_err(io_error)?,
        )?;
    }
    let copied = environment_inventory(destination)?;
    if copied != files {
        return Err(
            "template workspace changed while its curated environment was copied".to_owned(),
        );
    }
    write_environment_manifest(destination, &copied)
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
                    sha256: sha256_file(&path)?,
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
                entry.sha256
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

/// Sets `name` to `url` on the repository at `path`, adding the remote if
/// it does not yet exist or updating its URL in place if it does. The URL
/// is treated opaquely: no scheme or forge inspection.
fn set_remote(path: &Path, name: &str, url: &str) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["remote", "add", name, url])
        .output()
        .map_err(io_error)?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if detail.contains("already exists") {
        run_git(
            Command::new("git")
                .arg("-C")
                .arg(path)
                .args(["remote", "set-url", name])
                .arg(url),
            "failed to update push remote",
        )
    } else {
        Err(format!("failed to set push remote: {detail}"))
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
        if entry.file_type().map_err(io_error)?.is_dir() && entry.file_name() != ".templates" {
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

fn validate_environment_path(path: &Path) -> Result<&Path> {
    validate_relative_path(path).map_err(|_| "environment path must be confined".to_owned())?;
    let value = path_to_slashes(path)?;
    if matches!(value.as_str(), "AGENTS.md" | "task.md" | "prompt.md")
        || value.starts_with("context/")
    {
        Ok(path)
    } else {
        Err("environment path must be AGENTS.md, task.md, prompt.md, or below context/".to_owned())
    }
}

fn path_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))
}

fn repository_revision(path: &Path) -> Result<String> {
    git_stdout(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "HEAD"]),
        "failed to resolve repository HEAD",
    )
}

fn try_repository_revision(path: &Path) -> Option<String> {
    repository_revision(path).ok()
}

fn reject_heimr_root_overlap(heimr_root: &Path, source: &Path) -> Result<()> {
    if heimr_root.starts_with(source) || source.starts_with(heimr_root) {
        Err("mounted source overlaps the Heimr root and would expose workspace or template metadata as worker-writable".to_owned())
    } else {
        Ok(())
    }
}

fn confined_directory(root: &Path, path: &Path, label: &str) -> Result<PathBuf> {
    let path = path.canonicalize().map_err(io_error)?;
    if path.is_dir() && path.starts_with(root) {
        Ok(path)
    } else {
        Err(format!(
            "{label} directory escapes the workspace: {}",
            path.display()
        ))
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
    write_atomic(path, content, || {
        if path.exists() {
            return Err(format!("refusing to overwrite {}", path.display()));
        }
        Ok(())
    })
}

/// Writes `content` to `path`, replacing any existing file, via
/// `write_atomic` with no overwrite precondition. See `write_atomic` for
/// the atomicity and failure-handling guarantees.
fn write_replacing(path: &Path, content: &[u8]) -> Result<()> {
    write_atomic(path, content, || Ok(()))
}

/// Shared body for atomic writes: writes `content` to a synced temporary
/// file next to `path`, then renames it over `path`. `precondition` runs
/// after the temporary file is synced but before the rename, so callers
/// can enforce an overwrite policy (or allow none) at the last possible
/// moment; on any failure the temporary file is removed and `path` is
/// left untouched.
fn write_atomic(
    path: &Path,
    content: &[u8],
    precondition: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let temporary = temporary_sibling(path);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(io_error)?;
        file.write_all(content).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        precondition()?;
        fs::rename(&temporary, path).map_err(io_error)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn temporary_sibling(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        TEMPORARY_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}
