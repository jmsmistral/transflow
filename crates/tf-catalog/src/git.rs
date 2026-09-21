//! Optional read-only Git provenance and isolated raw-object captures. No checkout/reset/stash.
use crate::{
    capture::{CaptureError, CaptureLimits, SourceSnapshot},
    source::{SourceIndex, hard_excluded},
    workspace::{Workspace, WorkspaceConfig, relative_path},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};
use tf_domain::BranchName;

/// Git errors are distinct from absence and omit arbitrary Git stderr/config contents.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// Existing Git metadata could not be read with the installed executable.
    #[error("Git metadata could not be read; check the repository and Git installation")]
    Repository,
    /// Process or filesystem failure.
    #[error("Git source selection could not access a required process or file")]
    Io(#[from] std::io::Error),
    /// A request needs an explicit data branch before any capture mutation.
    #[error("Detached HEAD or an explicit Git ref requires an explicit output data branch")]
    BranchRequired,
    /// Git unavailable for an explicit ref request.
    #[error("An explicit Git ref requires a Git repository")]
    NoRepository,
    /// Source at the selected ref is not a valid matching workspace.
    #[error(
        "The selected Git ref does not contain a valid matching workspace or supported source tree"
    )]
    Source,
    /// Bounded Git command failed to finish.
    #[error("Git inspection exceeded its time or output limit")]
    Limit,
    /// Exact copied source failed validation.
    #[error(transparent)]
    Capture(#[from] CaptureError),
}
/// Optional provenance, independent of copied bytes and data-branch ownership.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitProvenance {
    repository: PathBuf,
    workspace_relative: PathBuf,
    branch: Option<String>,
    commit: Option<String>,
    requested_ref: Option<String>,
    pre_command_dirty_paths: Vec<String>,
    registry_overlay: Option<RegistryOverlay>,
}
/// Tool-authored registry changes are separate from pre-command dirty files.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryOverlay {
    before_sha256: String,
    after_sha256: String,
}
impl GitProvenance {
    /// Attached symbolic branch; None means a detached commit.
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }
    /// Exact commit; None is a validated unborn symbolic branch.
    pub fn commit(&self) -> Option<&str> {
        self.commit.as_deref()
    }
    /// Explicit selector retained separately from the once-resolved commit.
    pub fn requested_ref(&self) -> Option<&str> {
        self.requested_ref.as_deref()
    }
    /// Dirty captured paths before Transflow reconciliation; values are not source contents.
    pub fn dirty_paths(&self) -> &[String] {
        &self.pre_command_dirty_paths
    }
    /// Workspace-relative location inside the owning repository.
    pub fn workspace_relative(&self) -> &Path {
        &self.workspace_relative
    }
    pub(crate) fn valid(&self) -> bool {
        self.repository.is_absolute()
            && !self.workspace_relative.is_absolute()
            && self
                .workspace_relative
                .components()
                .all(|p| matches!(p, std::path::Component::Normal(_)))
            && self.commit.as_ref().is_none_or(|s| oid(s))
            && (self.commit.is_some() || self.branch.is_some())
            && self
                .branch
                .as_ref()
                .is_none_or(|s| s.parse::<BranchName>().is_ok())
            && self.pre_command_dirty_paths.len() <= 100_004
            && self
                .registry_overlay
                .as_ref()
                .is_none_or(|o| hex(&o.before_sha256, 64) && hex(&o.after_sha256, 64))
    }
}
fn hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn oid(s: &str) -> bool {
    hex(s, 40) || hex(s, 64)
}
fn command(root: &Path) -> Command {
    let mut c = Command::new("git");
    // Plumbing commands need no user hooks, credential helpers, pager, filters or network.
    c.env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("LC_ALL", "C")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "protocol.allow=never",
        ])
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    c
}
fn run(root: &Path, args: &[&str], limit: u64) -> Result<(ExitStatus, Vec<u8>), GitError> {
    let mut child = command(root).args(args).stdout(Stdio::piped()).spawn()?;
    let stdout = child.stdout.take().ok_or(GitError::Repository)?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if start.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(GitError::Limit);
        }
        thread::sleep(Duration::from_millis(10));
    };
    let bytes = reader.join().map_err(|_| GitError::Repository)??;
    if bytes.len() as u64 > limit {
        return Err(GitError::Limit);
    }
    Ok((status, bytes))
}
fn output(root: &Path, args: &[&str], limit: u64) -> Result<Vec<u8>, GitError> {
    let (status, bytes) = run(root, args, limit)?;
    if !status.success() {
        return Err(GitError::Repository);
    }
    Ok(bytes)
}
fn text(root: &Path, args: &[&str]) -> Result<String, GitError> {
    let bytes = output(root, args, 64 * 1024)?;
    let s = String::from_utf8(bytes).map_err(|_| GitError::Repository)?;
    Ok(s.trim_end_matches('\n').to_owned())
}
/// Inspect only ancestors for Git ownership. No marker means no Git executable is needed.
pub fn inspect(workspace: &Workspace) -> Result<Option<GitProvenance>, GitError> {
    inspect_metadata(workspace, true)
}
fn inspect_metadata(
    workspace: &Workspace,
    include_dirty: bool,
) -> Result<Option<GitProvenance>, GitError> {
    let mut found = false;
    for ancestor in workspace.root().ancestors() {
        match fs::symlink_metadata(ancestor.join(".git")) {
            Ok(_) => {
                found = true;
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    if !found {
        return Ok(None);
    }
    let repository =
        PathBuf::from(text(workspace.root(), &["rev-parse", "--show-toplevel"])?).canonicalize()?;
    let relative = workspace
        .root()
        .strip_prefix(&repository)
        .map_err(|_| GitError::Repository)?
        .to_owned();
    let (status, branch) = run(
        workspace.root(),
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        64 * 1024,
    )?;
    let branch = match status.code() {
        Some(0) => Some(
            String::from_utf8(branch)
                .map_err(|_| GitError::Repository)?
                .trim_end_matches('\n')
                .to_owned(),
        ),
        Some(1) => None,
        _ => return Err(GitError::Repository),
    };
    let (status, commit) = run(
        workspace.root(),
        &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
        1024,
    )?;
    let commit = if status.success() {
        Some(
            String::from_utf8(commit)
                .map_err(|_| GitError::Repository)?
                .trim()
                .to_owned(),
        )
    } else if status.code() == Some(1) && branch.is_some() {
        let reference = format!(
            "refs/heads/{}",
            branch.as_deref().ok_or(GitError::Repository)?
        );
        let (s, _) = run(
            workspace.root(),
            &["show-ref", "--verify", "--quiet", &reference],
            1024,
        )?;
        if s.code() != Some(1) {
            return Err(GitError::Repository);
        }
        None
    } else {
        return Err(GitError::Repository);
    };
    let paths = if include_dirty {
        let mut allowed: BTreeSet<_> = SourceIndex::enumerate(workspace)
            .map_err(CaptureError::from)?
            .files()
            .iter()
            .map(|f| f.path().to_owned())
            .collect();
        let (input, lock) = workspace.config().dependency_paths();
        allowed.extend([
            PathBuf::from("workspace.toml"),
            PathBuf::from(".transflow/catalog.toml"),
            PathBuf::from(input),
            PathBuf::from(lock),
        ]);
        let dirty = output(
            workspace.root(),
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignore-submodules=all",
                "--",
                ".",
            ],
            16 * 1024 * 1024,
        )?;
        let mut paths = BTreeSet::new();
        let mut records = dirty.split(|b| *b == 0).filter(|r| !r.is_empty());
        while let Some(record) = records.next() {
            if record.len() < 4 {
                return Err(GitError::Repository);
            }
            let raw = std::str::from_utf8(&record[3..]).map_err(|_| GitError::Repository)?;
            let path = Path::new(raw)
                .strip_prefix(&relative)
                .map_err(|_| GitError::Repository)?;
            if allowed.contains(path)
                || (workspace
                    .config()
                    .source_roots()
                    .iter()
                    .any(|root| path.starts_with(root))
                    && !hard_excluded(path)
                    && !crate::source::excluded_by_patterns(
                        workspace.config().source_exclude(),
                        path,
                    )
                    .map_err(CaptureError::from)?)
            {
                paths.insert(path.to_string_lossy().into_owned());
            }
            if record[..2].contains(&b'R') || record[..2].contains(&b'C') {
                records.next().ok_or(GitError::Repository)?;
            }
        }
        paths
    } else {
        BTreeSet::new()
    };
    let p = GitProvenance {
        repository,
        workspace_relative: relative,
        branch,
        commit,
        requested_ref: None,
        pre_command_dirty_paths: paths.into_iter().collect(),
        registry_overlay: None,
    };
    if !p.valid() {
        return Err(GitError::Repository);
    }
    Ok(Some(p))
}
/// Select a data branch without changing Git, creating database branches or copying heads.
pub fn output_branch(
    config: &WorkspaceConfig,
    git: Option<&GitProvenance>,
    explicit: Option<&BranchName>,
    explicit_ref: bool,
) -> Result<BranchName, GitError> {
    Ok(select_output_branch(config, git, explicit, None, explicit_ref)?.name)
}
/// Why this output name was selected; distinct from input fallback provenance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputBranchOrigin {
    /// Explicit CLI/API data-branch override.
    Explicit,
    /// Frozen accepted request or schedule revision.
    Recorded,
    /// Attached or unborn symbolic Git HEAD.
    Git,
    /// Non-Git or follow-Git-disabled workspace default.
    Default,
}
/// Once-selected output context. It performs no branch creation or head copying.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputBranchSelection {
    /// Exact opaque data-branch name.
    pub name: BranchName,
    /// Selection rule used by this request.
    pub origin: OutputBranchOrigin,
}
/// Select explicit, recorded, attached/unborn Git, or configured default in that order.
/// Git inspection failures must propagate from `inspect`; never convert them to absence.
/// A recorded request need not inspect the operator's current checkout at all.
pub fn select_output_branch(
    config: &WorkspaceConfig,
    git: Option<&GitProvenance>,
    explicit: Option<&BranchName>,
    recorded: Option<&BranchName>,
    explicit_ref: bool,
) -> Result<OutputBranchSelection, GitError> {
    let (name, origin) = if let Some(name) = explicit {
        (name.clone(), OutputBranchOrigin::Explicit)
    } else if let Some(name) = recorded {
        (name.clone(), OutputBranchOrigin::Recorded)
    } else if explicit_ref {
        return Err(GitError::BranchRequired);
    } else if config.follow_git_branch()
        && let Some(git) = git
    {
        (
            git.branch
                .as_ref()
                .ok_or(GitError::BranchRequired)?
                .parse()
                .map_err(|_| GitError::Repository)?,
            OutputBranchOrigin::Git,
        )
    } else {
        (
            config
                .default_branch()
                .parse()
                .map_err(|_| GitError::Source)?,
            OutputBranchOrigin::Default,
        )
    };
    Ok(OutputBranchSelection { name, origin })
}

/// Capture the current working tree with optional pre-command Git provenance.
pub fn capture_working_tree(
    workspace: &Workspace,
    limits: CaptureLimits,
) -> Result<SourceSnapshot, GitError> {
    let git = inspect(workspace)?;
    Ok(SourceSnapshot::capture_internal(
        workspace,
        limits,
        git,
        None,
        |_| Ok(()),
    )?)
}
/// Capture working-tree provenance in an exclusively owned temporary directory.
pub fn capture_temporary(
    workspace: &Workspace,
    destination: &Path,
    limits: CaptureLimits,
) -> Result<SourceSnapshot, GitError> {
    Ok(SourceSnapshot::capture_internal(
        workspace,
        limits,
        inspect(workspace)?,
        Some(destination),
        |_| Ok(()),
    )?)
}
#[derive(Clone)]
struct Blob {
    mode: String,
    oid: String,
    size: u64,
}
/// Resolve a ref once and copy raw allowed blobs into a private managed tree before capture.
/// Filters, export attributes, hooks and user checkout mutations are never involved.
pub fn capture_ref(
    workspace: &Workspace,
    selector: &str,
    branch: Option<&BranchName>,
    limits: CaptureLimits,
) -> Result<SourceSnapshot, GitError> {
    output_branch(workspace.config(), None, branch, true)?;
    if selector.is_empty()
        || selector.len() > 4096
        || selector.starts_with('-')
        || selector.chars().any(char::is_control)
    {
        return Err(GitError::Source);
    }
    let mut provenance = inspect_metadata(workspace, false)?.ok_or(GitError::NoRepository)?;
    let commit = text(
        &provenance.repository,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{selector}^{{commit}}"),
        ],
    )?;
    if !oid(&commit) {
        return Err(GitError::Repository);
    }
    let config_path = provenance.workspace_relative.join("workspace.toml");
    let config_spec = format!("{commit}:{}", config_path.to_str().ok_or(GitError::Source)?);
    let config_bytes = output(
        &provenance.repository,
        &["cat-file", "blob", &config_spec],
        1024 * 1024,
    )?;
    let config_text = std::str::from_utf8(&config_bytes).map_err(|_| GitError::Source)?;
    let config = WorkspaceConfig::parse(config_text).map_err(|_| GitError::Source)?;
    if config.id() != workspace.config().id() {
        return Err(GitError::Source);
    }
    let (input, lock) = config.dependency_paths();
    let authoring = [
        PathBuf::from("workspace.toml"),
        PathBuf::from(".transflow/catalog.toml"),
        PathBuf::from(input),
        PathBuf::from(lock),
    ];
    if authoring
        .iter()
        .any(|p| p != Path::new(".transflow/catalog.toml") && hard_excluded(p))
    {
        return Err(GitError::Source);
    }
    let selection: Vec<_> = config
        .source_roots()
        .iter()
        .chain(authoring.iter())
        .map(|p| provenance.workspace_relative.join(p))
        .collect();
    let mut args = vec!["ls-tree", "-r", "-z", "-l", "--full-tree", &commit, "--"];
    for path in &selection {
        args.push(path.to_str().ok_or(GitError::Source)?);
    }
    let listing = output(&provenance.repository, &args, 16 * 1024 * 1024)?;
    let mut blobs = BTreeMap::new();
    for record in listing.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let record = std::str::from_utf8(record).map_err(|_| GitError::Source)?;
        let (meta, path) = record.split_once('\t').ok_or(GitError::Source)?;
        let fields: Vec<_> = meta.split_whitespace().collect();
        if fields.len() != 4 {
            return Err(GitError::Source);
        }
        let relative = Path::new(path)
            .strip_prefix(&provenance.workspace_relative)
            .map_err(|_| GitError::Source)?
            .to_owned();
        if !authoring.contains(&relative)
            && (hard_excluded(&relative)
                || crate::source::excluded_by_patterns(config.source_exclude(), &relative)
                    .map_err(CaptureError::from)?)
        {
            continue;
        }
        if relative != Path::new(".transflow/catalog.toml") {
            relative_path(relative.to_str().ok_or(GitError::Source)?)
                .map_err(|_| GitError::Source)?;
        }
        if !oid(fields[2]) {
            return Err(GitError::Source);
        }
        let size = fields[3].parse().map_err(|_| GitError::Source)?;
        blobs.insert(
            relative,
            Blob {
                mode: fields[0].into(),
                oid: fields[2].into(),
                size,
            },
        );
        if blobs.len() > 100_004 {
            return Err(GitError::Limit);
        }
    }
    let nested: Vec<_> = blobs
        .keys()
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n == "workspace.toml" || n == "pyvenv.cfg")
                && !authoring.contains(p)
        })
        .filter_map(|p| p.parent().map(Path::to_owned))
        .collect();
    blobs.retain(|path, _| !nested.iter().any(|root| path.starts_with(root)));
    let runtime = workspace.root().join(".transflow/runtime");
    if !fs::symlink_metadata(&runtime)?.is_dir()
        || fs::symlink_metadata(&runtime)?.file_type().is_symlink()
    {
        return Err(GitError::Source);
    }
    let mut random = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut random)?;
    let name: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let managed = runtime.join(format!("git-capture-{name}"));
    fs::DirBuilder::new().mode(0o700).create(&managed)?;
    let result = (|| {
        let mut total = 0u64;
        for (path, blob) in blobs {
            if blob.size > limits.file_bytes {
                return Err(GitError::Limit);
            }
            total = total.checked_add(blob.size).ok_or(GitError::Limit)?;
            if total > limits.total_bytes {
                return Err(GitError::Limit);
            }
            let destination = managed.join(&path);
            fs::create_dir_all(destination.parent().ok_or(GitError::Source)?)?;
            match blob.mode.as_str() {
                "100644" | "100755" => {
                    let f = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&destination)?;
                    let mut child = command(&provenance.repository)
                        .args(["cat-file", "blob", &blob.oid])
                        .stdout(Stdio::from(f.try_clone()?))
                        .spawn()?;
                    let start = Instant::now();
                    let status = loop {
                        if let Some(status) = child.try_wait()? {
                            break status;
                        }
                        if start.elapsed() > Duration::from_secs(30) {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err(GitError::Limit);
                        }
                        thread::sleep(Duration::from_millis(10));
                    };
                    if !status.success() || f.metadata()?.len() != blob.size {
                        return Err(GitError::Source);
                    }
                }
                "120000" => {
                    let bytes = output(
                        &provenance.repository,
                        &["cat-file", "blob", &blob.oid],
                        4096,
                    )?;
                    let target = std::str::from_utf8(&bytes).map_err(|_| GitError::Source)?;
                    if Path::new(target).is_absolute() {
                        return Err(GitError::Source);
                    }
                    std::os::unix::fs::symlink(target, &destination)?;
                }
                _ => return Err(GitError::Source),
            }
        }
        for root in config.source_roots() {
            fs::create_dir_all(managed.join(root))?;
        }
        fs::create_dir_all(managed.join(".transflow/runtime"))?;
        let frozen = Workspace::load(&managed, Some(&managed)).map_err(|_| GitError::Source)?;
        provenance.commit = Some(commit);
        provenance.requested_ref = Some(selector.to_owned());
        provenance.branch = None;
        // The selected immutable commit is clean; caller working-tree dirt is irrelevant to its bytes.
        provenance.pre_command_dirty_paths.clear();
        Ok(SourceSnapshot::capture_internal(
            &frozen,
            limits,
            Some(provenance),
            Some(&runtime),
            |_| Ok(()),
        )?)
    })();
    let cleanup = fs::remove_dir_all(&managed);
    match result {
        Err(e) => Err(e),
        Ok(s) => {
            cleanup?;
            Ok(s)
        }
    }
}
