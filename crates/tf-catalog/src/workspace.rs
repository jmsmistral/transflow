//! Strict workspace policy, explicit local locators and root selection. No Python or Git.
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};
use tf_domain::{BranchName, WorkspaceId};

/// A safe configuration diagnostic: user values and parser source excerpts are omitted.
#[derive(Debug, thiserror::Error)]
#[error(
    "Invalid workspace configuration at {field}, {location} ({reason}); correct this setting and retry"
)]
pub struct ConfigError {
    /// Fixed schema field or filename.
    pub field: &'static str,
    location: String,
    /// Fixed explanation, never raw configuration contents.
    pub reason: &'static str,
    /// TOML byte range when the parser can locate the error.
    pub span: Option<std::ops::Range<usize>>,
}
fn invalid(field: &'static str, reason: &'static str) -> ConfigError {
    ConfigError {
        field,
        location: "setting validation".into(),
        reason,
        span: None,
    }
}
/// Configuration result.
pub type ConfigResult<T> = Result<T, ConfigError>;
fn decode<T: serde::de::DeserializeOwned>(text: &str, field: &'static str) -> ConfigResult<T> {
    if text.len() > 1024 * 1024 {
        return Err(invalid(field, "file exceeds 1 MiB"));
    }
    toml::from_str(text).map_err(|e: toml::de::Error| ConfigError {
        field,
        location: e
            .span()
            .map(|span| {
                let prefix = text.get(..span.start).unwrap_or("");
                format!(
                    "line {}, column {}",
                    prefix.bytes().filter(|b| *b == b'\n').count() + 1,
                    prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1
                )
            })
            .unwrap_or_else(|| "TOML document".into()),
        reason: "unknown key, invalid type or malformed TOML",
        span: e.span(),
    })
}
/// Read a bounded regular authoring file; reject symlinks and special files.
pub fn read_authoring(path: &Path) -> ConfigResult<String> {
    let m = fs::symlink_metadata(path)
        .map_err(|_| invalid("authoring file", "file is missing or unreadable"))?;
    if !m.is_file() || m.file_type().is_symlink() || m.len() > 16 * 1024 * 1024 {
        return Err(invalid(
            "authoring file",
            "expected a regular file of at most 16 MiB",
        ));
    }
    let mut text = String::new();
    let fd = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| invalid("authoring file", "cannot safely open file"))?;
    let file = fs::File::from(fd);
    let opened = file
        .metadata()
        .map_err(|_| invalid("authoring file", "cannot inspect opened file"))?;
    use std::os::unix::fs::MetadataExt;
    if !opened.is_file() || opened.dev() != m.dev() || opened.ino() != m.ino() {
        return Err(invalid(
            "authoring file",
            "file changed while opening; retry",
        ));
    }
    file.take(16 * 1024 * 1024 + 1)
        .read_to_string(&mut text)
        .map_err(|_| invalid("authoring file", "cannot read UTF-8 text"))?;
    if text.len() > 16 * 1024 * 1024 {
        return Err(invalid("authoring file", "file exceeds 16 MiB"));
    }
    Ok(text)
}
/// Validate a nonempty relative path without parent components or protected directories.
pub fn relative_path(text: &str) -> ConfigResult<PathBuf> {
    let path = Path::new(text);
    if text.is_empty()
        || text.len() > 4096
        || text.contains('\\')
        || text.chars().any(char::is_control)
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || path.components().any(|c| {
            matches!(
                c.as_os_str().to_str(),
                Some(".git" | ".transflow" | ".venv" | "venv")
            )
        })
    {
        return Err(invalid(
            "path",
            "use a workspace-relative path outside runtime, Git and environments",
        ));
    }
    Ok(path.to_owned())
}
macro_rules! section {
    ($name:ident { $($field:ident : $ty:ty = $value:expr),* $(,)? }) => {
        #[derive(Clone, Debug, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        struct $name { $($field: $ty),* }
        impl Default for $name { fn default() -> Self { Self { $($field: $value),* } } }
    }
}
section!(Python {
    version: String = "3.14".into(),
    requirements: String = "requirements.in".into(),
    lock: String = "requirements.lock".into()
});
section!(Branching { follow_git_branch: bool = true, default_fallbacks: Vec<String> = vec!["master".into()], fallbacks: BTreeMap<String, Vec<String>> = BTreeMap::new() });
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum Cpu {
    Count(u32),
    Auto(String),
}
section!(Execution { max_jobs: u32 = 2, cpu_tokens: Cpu = Cpu::Auto("auto".into()), wall_timeout_seconds: u64 = 3600, memory_budget_mib: Option<u64> = None });
section!(Validation {
    null_policy: String = "fail".into(),
    sample_rows: u32 = 0,
    timeout_seconds: u64 = 3600
});
section!(Interactive {
    query_timeout_seconds: u64 = 30,
    preview_rows: u32 = 100,
    max_result_rows: u32 = 1000,
    max_result_bytes: u64 = 2097152
});
section!(Retention {
    keep_versions_per_dataset_branch: u32 = 20,
    keep_days: u32 = 30,
    quarantine_days: u32 = 7
});
section!(Security {
    listen: String = "127.0.0.1".into(),
    telemetry: bool = false
});
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    format_version: u32,
    workspace_id: String,
    #[serde(default = "roots")]
    source_roots: Vec<String>,
    #[serde(default)]
    source_exclude: Vec<String>,
    #[serde(default = "master")]
    default_data_branch: String,
    #[serde(default)]
    python: Python,
    #[serde(default)]
    branching: Branching,
    #[serde(default)]
    execution: Execution,
    #[serde(default)]
    validation: Validation,
    #[serde(default)]
    interactive: Interactive,
    #[serde(default)]
    retention: Retention,
    #[serde(default)]
    security: Security,
}
fn roots() -> Vec<String> {
    vec!["src".into()]
}
fn master() -> String {
    "master".into()
}
/// Validated immutable workspace authoring policy.
#[derive(Clone, Debug)]
pub struct WorkspaceConfig {
    raw: Raw,
    id: WorkspaceId,
    source_roots: Vec<PathBuf>,
    explicit: toml::Value,
}
impl WorkspaceConfig {
    /// Parse with defaults, strict keys/types and no environment-dependent policy overrides.
    pub fn parse(text: &str) -> ConfigResult<Self> {
        let raw: Raw = decode(text, "workspace.toml")?;
        if raw.format_version != 1 {
            return Err(invalid(
                "format_version",
                "unsupported format; use a compatible application",
            ));
        }
        let id = raw
            .workspace_id
            .parse()
            .map_err(|_| invalid("workspace_id", "expected a UUID"))?;
        let source_roots: Vec<_> = raw
            .source_roots
            .iter()
            .map(|s| relative_path(s))
            .collect::<Result<_, _>>()?;
        if source_roots.is_empty() || source_roots.len() > 128 {
            return Err(invalid("source_roots", "expected 1 to 128 roots"));
        }
        for (i, root) in source_roots.iter().enumerate() {
            if source_roots
                .iter()
                .skip(i + 1)
                .any(|other| root.starts_with(other) || other.starts_with(root))
            {
                return Err(invalid("source_roots", "duplicate or overlapping roots"));
            }
        }
        if raw.source_exclude.len() > 1024
            || raw.source_exclude.iter().any(|p| {
                p.is_empty()
                    || p.len() > 4096
                    || p.starts_with('/')
                    || p.split('/').any(|s| s == "..")
                    || p.chars().any(char::is_control)
            })
        {
            return Err(invalid(
                "source_exclude",
                "invalid or excessive exclusion globs",
            ));
        }
        for branch in std::iter::once(&raw.default_data_branch)
            .chain(raw.branching.default_fallbacks.iter())
            .chain(raw.branching.fallbacks.keys())
            .chain(raw.branching.fallbacks.values().flatten())
        {
            if branch.len() > 4096 || branch.parse::<BranchName>().is_err() {
                return Err(invalid("branching", "invalid branch name"));
            }
        }
        if !matches!(raw.python.version.as_str(), "3.13" | "3.14") {
            return Err(invalid(
                "python.version",
                "supported versions are 3.13 and 3.14",
            ));
        }
        relative_path(&raw.python.requirements)?;
        relative_path(&raw.python.lock)?;
        if raw.python.requirements == raw.python.lock {
            return Err(invalid(
                "python.lock",
                "lock and dependency input must be different files",
            ));
        }
        if raw.execution.max_jobs == 0 || raw.execution.memory_budget_mib == Some(0) {
            return Err(invalid(
                "execution",
                "job count and an explicit memory budget must be positive",
            ));
        }
        match &raw.execution.cpu_tokens {
            Cpu::Count(0) => {
                return Err(invalid(
                    "execution.cpu_tokens",
                    "expected auto or a positive count",
                ));
            }
            Cpu::Auto(v) if v != "auto" => {
                return Err(invalid(
                    "execution.cpu_tokens",
                    "expected auto or a positive count",
                ));
            }
            _ => {}
        }
        if !matches!(raw.validation.null_policy.as_str(), "fail" | "ignore") {
            return Err(invalid("validation", "null policy must be fail or ignore"));
        }
        if raw.interactive.preview_rows == 0
            || raw.interactive.max_result_rows < raw.interactive.preview_rows
            || raw.interactive.max_result_bytes == 0
        {
            return Err(invalid(
                "interactive",
                "result limits must be positive and include the preview",
            ));
        }
        if raw.retention.keep_versions_per_dataset_branch == 0 {
            return Err(invalid(
                "retention.keep_versions_per_dataset_branch",
                "must retain at least one version",
            ));
        }
        if raw.security.telemetry
            || !raw
                .security
                .listen
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        {
            return Err(invalid(
                "security",
                "only loopback listening and disabled telemetry are supported",
            ));
        }
        let explicit = decode(text, "workspace.toml")?;
        Ok(Self {
            raw,
            id,
            source_roots,
            explicit,
        })
    }
    /// Stable authoring workspace identity.
    pub fn id(&self) -> WorkspaceId {
        self.id
    }
    /// Ordered relative source/import roots.
    pub fn source_roots(&self) -> &[PathBuf] {
        &self.source_roots
    }
    /// User source exclusion globs (hard exclusions always apply independently).
    pub fn source_exclude(&self) -> &[String] {
        &self.raw.source_exclude
    }
    /// Configured default data branch, independent of source selection.
    pub fn default_branch(&self) -> &str {
        &self.raw.default_data_branch
    }
    /// Whether ordinary builds follow an attached Git branch.
    pub fn follow_git_branch(&self) -> bool {
        self.raw.branching.follow_git_branch
    }
    /// Ordered starting-branch policy; the caller avoids revisiting the starting branch.
    pub fn fallbacks(&self, branch: &str) -> &[String] {
        self.raw
            .branching
            .fallbacks
            .get(branch)
            .unwrap_or(&self.raw.branching.default_fallbacks)
    }
    /// Capture authored local branch rules, preserving explicit empty tails.
    pub fn input_policies(
        &self,
    ) -> Result<tf_domain::input::BranchPolicySnapshot, tf_domain::input::InputError> {
        use tf_domain::input::{BranchPolicySnapshot, InputError};
        let parse = |names: &[String]| {
            names
                .iter()
                .map(|n| n.parse().map_err(|_| InputError))
                .collect::<Result<Vec<_>, _>>()
        };
        let rules = self
            .raw
            .branching
            .fallbacks
            .iter()
            .map(|(n, tail)| Ok((n.parse().map_err(|_| InputError)?, parse(tail)?)))
            .collect::<Result<_, InputError>>()?;
        BranchPolicySnapshot::new(parse(&self.raw.branching.default_fallbacks)?, rules)
    }
    /// Configured Python minor version.
    pub fn python_version(&self) -> &str {
        &self.raw.python.version
    }
    /// Relative dependency input and hash lock paths.
    pub fn dependency_paths(&self) -> (&str, &str) {
        (&self.raw.python.requirements, &self.raw.python.lock)
    }
    /// Validation null policy.
    pub fn null_policy(&self) -> &str {
        &self.raw.validation.null_policy
    }
    /// Maximum diagnostic sample rows; evaluation remains exact over all rows.
    pub fn sample_rows(&self) -> u32 {
        self.raw.validation.sample_rows
    }
    /// Preview rows, maximum rows and maximum bytes.
    pub fn interactive_limits(&self) -> (u32, u32, u64) {
        (
            self.raw.interactive.preview_rows,
            self.raw.interactive.max_result_rows,
            self.raw.interactive.max_result_bytes,
        )
    }
    /// Version count, age and quarantine retention defaults.
    pub fn retention(&self) -> (u32, u32, u32) {
        (
            self.raw.retention.keep_versions_per_dataset_branch,
            self.raw.retention.keep_days,
            self.raw.retention.quarantine_days,
        )
    }
    /// Resolve field-specific allowed precedence, preserving zero deadlines and absent memory.
    pub fn execution_policy(
        &self,
        definition: &DefinitionOverrides,
        build: &BuildOverrides,
        explicit: &BuildOverrides,
    ) -> ConfigResult<EffectivePolicy> {
        let value = |section: &str, field: &str, n| Setting {
            value: n,
            origin: if self
                .explicit
                .get(section)
                .and_then(|s| s.get(field))
                .is_some()
            {
                Origin::Workspace
            } else {
                Origin::Default
            },
        };
        let mut p = EffectivePolicy {
            transform_seconds: value(
                "execution",
                "wall_timeout_seconds",
                self.raw.execution.wall_timeout_seconds,
            ),
            validation_seconds: value(
                "validation",
                "timeout_seconds",
                self.raw.validation.timeout_seconds,
            ),
            interactive_seconds: value(
                "interactive",
                "query_timeout_seconds",
                self.raw.interactive.query_timeout_seconds,
            ),
            max_jobs: value(
                "execution",
                "max_jobs",
                u64::from(self.raw.execution.max_jobs),
            ),
            cpu_tokens: match self.raw.execution.cpu_tokens {
                Cpu::Count(n) => n,
                Cpu::Auto(_) => std::thread::available_parallelism()
                    .map_err(|_| {
                        invalid(
                            "execution.cpu_tokens",
                            "cannot detect CPUs; set an explicit count",
                        )
                    })?
                    .get()
                    .try_into()
                    .map_err(|_| invalid("execution.cpu_tokens", "CPU count out of range"))?,
            },
            memory_budget_mib: self
                .raw
                .execution
                .memory_budget_mib
                .map(|n| value("execution", "memory_budget_mib", n)),
        };
        if let Some(n) = definition.transform_seconds {
            p.transform_seconds = Setting {
                value: n,
                origin: Origin::Definition,
            };
        }
        for (o, origin) in [(build, Origin::Build), (explicit, Origin::Explicit)] {
            if let Some(n) = o.transform_seconds {
                p.transform_seconds = Setting { value: n, origin };
            }
            if let Some(n) = o.validation_seconds {
                p.validation_seconds = Setting { value: n, origin };
            }
            if let Some(n) = o.max_jobs {
                if n == 0 {
                    return Err(invalid("max_jobs", "must be positive"));
                }
                p.max_jobs = Setting {
                    value: u64::from(n),
                    origin,
                };
            }
        }
        Ok(p)
    }
}
/// Source of a resolved computation setting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Origin {
    /// Built-in default.
    Default,
    /// Workspace authoring policy.
    Workspace,
    /// Producer definition.
    Definition,
    /// Accepted build or schedule policy.
    Build,
    /// Explicit CLI/API override.
    Explicit,
}
/// A resolved integer and its provenance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Setting {
    /// Effective value (zero timeout disables its deadline).
    pub value: u64,
    /// Winning layer.
    pub origin: Origin,
}
/// Only the producer's transform deadline may be overridden at definition level here.
#[derive(Default)]
pub struct DefinitionOverrides {
    /// Transform execution/materialization timeout.
    pub transform_seconds: Option<u64>,
}
/// Typed permitted build/schedule/explicit policy overrides; no generic key merging.
#[derive(Default)]
pub struct BuildOverrides {
    /// Transform deadline.
    pub transform_seconds: Option<u64>,
    /// Per-validation-phase deadline.
    pub validation_seconds: Option<u64>,
    /// Active producer capacity.
    pub max_jobs: Option<u32>,
}
/// Resolved execution settings. Branch and pin precedence remain separate services.
#[derive(Debug)]
pub struct EffectivePolicy {
    /// Transformation deadline.
    pub transform_seconds: Setting,
    /// Independent input/output validation deadline.
    pub validation_seconds: Setting,
    /// Independent interactive deadline.
    pub interactive_seconds: Setting,
    /// Maximum active jobs.
    pub max_jobs: Setting,
    /// Resolved positive CPU count.
    pub cpu_tokens: u32,
    /// Absent means no Transflow memory admission gate.
    pub memory_budget_mib: Option<Setting>,
}

/// Machine-local locators. Deliberately no Debug implementation or computation policy keys.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LocalConfig {
    python: LocalPython,
    external_workspaces: BTreeMap<String, PathBuf>,
    secret_references: BTreeMap<String, String>,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LocalPython {
    executable: Option<PathBuf>,
}
impl LocalConfig {
    /// Parse local-only absolute paths and opaque secret *references*, never secret values.
    pub fn parse(text: &str) -> ConfigResult<Self> {
        let value: Self = decode(text, "transflow.local.toml")?;
        if value
            .python
            .executable
            .iter()
            .chain(value.external_workspaces.values())
            .any(|p| !p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)))
        {
            return Err(invalid(
                "transflow.local.toml",
                "locators must be absolute paths without parent components",
            ));
        }
        if value.secret_references.values().any(|s| {
            !s.starts_with("env:")
                || s.len() <= 4
                || !s[4..]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
        }) {
            return Err(invalid(
                "secret_references",
                "use env:VARIABLE references, not secret values",
            ));
        }
        for key in value.external_workspaces.keys() {
            key.parse::<WorkspaceId>().map_err(|_| {
                invalid(
                    "external_workspaces",
                    "keys must be provider workspace UUIDs",
                )
            })?;
        }
        Ok(value)
    }
    /// Interpreter locator; the environment service must report and fingerprint its identity.
    pub fn interpreter(&self) -> Option<&Path> {
        self.python.executable.as_deref()
    }
    /// Explicit provider locator, never a global search by UUID.
    pub fn provider(&self, id: WorkspaceId) -> Option<&Path> {
        self.external_workspaces
            .get(&id.to_string())
            .map(PathBuf::as_path)
    }
}
/// Explicit root wins; otherwise walk ancestor directories and choose the nearest config.
/// This checks filenames only, never enumerates siblings or imports Python.
pub fn select_workspace(start: &Path, explicit: Option<&Path>) -> ConfigResult<PathBuf> {
    let selected = explicit
        .unwrap_or(start)
        .canonicalize()
        .map_err(|_| invalid("workspace", "directory does not exist"))?;
    if !selected.is_dir() {
        return Err(invalid("workspace", "expected a directory"));
    }
    if explicit.is_some() {
        if selected.join("workspace.toml").is_file() {
            return Ok(selected);
        }
        return Err(invalid(
            "workspace",
            "explicit directory has no workspace.toml",
        ));
    }
    for root in selected.ancestors() {
        if root.join("workspace.toml").exists() {
            return Ok(root.to_owned());
        }
    }
    Err(invalid(
        "workspace",
        "no workspace.toml found; run transflow init",
    ))
}

/// Selected, validated authoring workspace and local locator layer. Loading does not create state.
pub struct Workspace {
    root: PathBuf,
    config: WorkspaceConfig,
    local: LocalConfig,
    roots: Vec<PathBuf>,
}
impl Workspace {
    /// Resolve context, parse both configuration layers, validate roots and registry without imports.
    pub fn load(start: &Path, explicit: Option<&Path>) -> ConfigResult<Self> {
        let root = select_workspace(start, explicit)?;
        let config = WorkspaceConfig::parse(&read_authoring(&root.join("workspace.toml"))?)?;
        let local_path = root.join("transflow.local.toml");
        let local = match fs::symlink_metadata(&local_path) {
            Ok(_) => LocalConfig::parse(&read_authoring(&local_path)?)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => LocalConfig::default(),
            Err(_) => {
                return Err(invalid(
                    "transflow.local.toml",
                    "cannot inspect local settings",
                ));
            }
        };
        let mut roots: Vec<PathBuf> = Vec::new();
        for relative in config.source_roots() {
            let resolved = root.join(relative).canonicalize().map_err(|_| {
                invalid(
                    "source_roots",
                    "a configured directory is missing or unreadable",
                )
            })?;
            if !resolved.is_dir() || !resolved.starts_with(&root) || resolved == root {
                return Err(invalid(
                    "source_roots",
                    "root must be a directory inside this workspace",
                ));
            }
            let rel = resolved
                .strip_prefix(&root)
                .map_err(|_| invalid("source_roots", "root escaped workspace"))?;
            relative_path(
                rel.to_str()
                    .ok_or_else(|| invalid("source_roots", "root must be UTF-8"))?,
            )?;
            for ancestor in resolved.ancestors().take_while(|p| *p != root) {
                if ancestor.join("pyvenv.cfg").exists() || ancestor.join("workspace.toml").exists()
                {
                    return Err(invalid(
                        "source_roots",
                        "root is inside an environment or another workspace",
                    ));
                }
            }
            if roots
                .iter()
                .any(|other| resolved.starts_with(other) || other.starts_with(&resolved))
            {
                return Err(invalid("source_roots", "resolved roots overlap"));
            }
            roots.push(resolved);
        }
        let authoring = fs::symlink_metadata(root.join(".transflow"))
            .map_err(|_| invalid(".transflow", "authoring directory is missing"))?;
        if !authoring.is_dir() || authoring.file_type().is_symlink() {
            return Err(invalid(
                ".transflow",
                "use an ordinary directory, not a symlink",
            ));
        }
        crate::RegistrySnapshot::parse(
            config.id(),
            &read_authoring(&root.join(".transflow/catalog.toml"))?,
        )
        .map_err(|_| {
            invalid(
                ".transflow/catalog.toml",
                "durable registry is missing or invalid",
            )
        })?;
        Ok(Self {
            root,
            config,
            local,
            roots,
        })
    }
    /// Canonical explicit workspace directory.
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Immutable validated computation settings.
    pub fn config(&self) -> &WorkspaceConfig {
        &self.config
    }
    /// Local locators; never merged into computation policies.
    pub fn local(&self) -> &LocalConfig {
        &self.local
    }
    /// Canonical configured roots, in authoring order.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }
}
