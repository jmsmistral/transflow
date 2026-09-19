use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt, ops::Range};
use tf_domain::{
    DatasetId, DatasetKey, DatasetPath, DatasetScope, ExternalRegistrationId, WorkspaceId,
    branch::BranchName,
};
use tf_protocol::canonical::{ContentDigest, DigestKind, content_digest, file_digest};
/// Maximum registry input size; TOML parsing never reads a filesystem implicitly.
pub const MAX_REGISTRY_BYTES: usize = 16 * 1024 * 1024;
/// Classification independent of raw TOML/error text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryErrorKind {
    /// Syntax, type or unknown field.
    Syntax,
    /// Unsupported registry version.
    Version,
    /// Invalid ID/path/kind/branch metadata.
    Invalid,
    /// Bounded input/record/path budget exceeded.
    Limit,
    /// Duplicate identity or exact name.
    Collision,
    /// Alias target is absent or tombstoned.
    AliasTarget,
    /// Exact reference is absent; lookup never registers it.
    Missing,
    /// Namespace exists but has no dataset identity.
    Namespace,
    /// Deleted identity/name remains reserved.
    Tombstone,
    /// A foreign registration cannot be an output.
    ForeignOutput,
}
/// Safe error with record locations and optional parser byte range; never echoes source text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistryError {
    kind: RegistryErrorKind,
    location: String,
    related: Option<String>,
    span: Option<Range<usize>>,
}
impl RegistryError {
    /// Stable machine classification.
    pub fn kind(&self) -> RegistryErrorKind {
        self.kind
    }
    /// JSON-pointer-style record location, without user-supplied names.
    pub fn location(&self) -> &str {
        &self.location
    }
    /// Earlier record involved in a collision.
    pub fn related_location(&self) -> Option<&str> {
        self.related.as_deref()
    }
    /// Syntax diagnostic byte range in the supplied TOML.
    pub fn span(&self) -> Option<Range<usize>> {
        self.span.clone()
    }
}
impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.kind {
            RegistryErrorKind::Syntax => {
                "The catalogue registry has invalid syntax, types or unknown fields"
            }
            RegistryErrorKind::Version => "This catalogue registry format is unsupported",
            RegistryErrorKind::Invalid => {
                "The catalogue record contains an invalid identity, name or policy"
            }
            RegistryErrorKind::Limit => "The catalogue registry exceeds its input or record limits",
            RegistryErrorKind::Collision => "Catalogue identities and names must be unique",
            RegistryErrorKind::AliasTarget => {
                "A catalogue alias must target an active local dataset"
            }
            RegistryErrorKind::Missing => "The exact dataset reference is not registered",
            RegistryErrorKind::Namespace => "The reference names a namespace without a dataset",
            RegistryErrorKind::Tombstone => "The dataset identity or name has been tombstoned",
            RegistryErrorKind::ForeignOutput => {
                "A foreign dataset registration cannot be an output"
            }
        })
    }
}
impl std::error::Error for RegistryError {}
/// Catalogue operation result.
pub type Result<T> = std::result::Result<T, RegistryError>;
fn error(kind: RegistryErrorKind, location: impl Into<String>) -> RegistryError {
    RegistryError {
        kind,
        location: location.into(),
        related: None,
        span: None,
    }
}
/// Registry kind contains no producer entrypoint or code-derived inputs/checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetKind {
    /// Ordinary transform.
    Transform,
    /// Source/API transform.
    Source,
    /// Imported immutable files, without a Python producer.
    Imported,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    format_version: u32,
    #[serde(default)]
    datasets: Vec<RawDataset>,
    #[serde(default)]
    aliases: Vec<RawAlias>,
    #[serde(default)]
    tombstones: Vec<RawDataset>,
    #[serde(default)]
    external_registrations: Vec<RawExternal>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawDataset {
    id: String,
    path: String,
    kind: DatasetKind,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawAlias {
    path: String,
    target_id: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawExternal {
    id: String,
    alias: String,
    provider_workspace_id: String,
    provider_dataset_id: String,
    provider_display_path: String,
    default_branch: String,
    fallback_override: Option<Vec<String>>,
}
/// Local durable identity, including retained deleted identities.
#[derive(Clone, Debug)]
pub struct LocalDataset {
    key: DatasetKey,
    path: DatasetPath,
    kind: DatasetKind,
    tombstone: bool,
}
impl LocalDataset {
    /// Owner-qualified identity independent of names and file locations.
    pub fn key(&self) -> DatasetKey {
        self.key
    }
    /// Current canonical path (or reserved tombstone path).
    pub fn path(&self) -> &DatasetPath {
        &self.path
    }
    /// Registry kind, without executable definition details.
    pub fn kind(&self) -> DatasetKind {
        self.kind
    }
    /// Whether this identity is retained only as a tombstone.
    pub fn is_tombstone(&self) -> bool {
        self.tombstone
    }
}
/// Explicit foreign read boundary. Machine-local locators are never stored here.
#[derive(Clone, Debug)]
pub struct ExternalRegistration {
    id: ExternalRegistrationId,
    alias: DatasetPath,
    key: DatasetKey,
    display_path: DatasetPath,
    branch: BranchName,
    fallback: Option<Vec<BranchName>>,
}
impl ExternalRegistration {
    /// Stable local registration identity, independent of provider dataset identity.
    pub fn id(&self) -> ExternalRegistrationId {
        self.id
    }
    /// Local path under external/.
    pub fn alias(&self) -> &DatasetPath {
        &self.alias
    }
    /// Exact provider workspace/dataset tuple.
    pub fn key(&self) -> DatasetKey {
        self.key
    }
    /// Non-authoritative provider display path.
    pub fn provider_display_path(&self) -> &DatasetPath {
        &self.display_path
    }
    /// Explicit default data branch captured at registration.
    pub fn default_branch(&self) -> &BranchName {
        &self.branch
    }
    /// None inherits provider policy later; Some(empty) explicitly overrides with no fallbacks.
    pub fn fallback_override(&self) -> Option<&[BranchName]> {
        self.fallback.as_deref()
    }
}
#[derive(Clone, Copy, Debug)]
enum Name {
    Local(DatasetId),
    Alias(DatasetId),
    Tombstone,
    External(ExternalRegistrationId),
}
/// Exact resolution, preserving the registration for policy-aware foreign planning.
#[derive(Clone, Copy, Debug)]
pub enum Resolved<'a> {
    /// Local dataset and whether the input spelling used a retained rename alias.
    Local {
        /// Canonical durable entry.
        dataset: &'a LocalDataset,
        /// Explicit alias, not fuzzy matching.
        via_alias: bool,
    },
    /// Registered foreign boundary; never eligible for producer execution.
    External(&'a ExternalRegistration),
}
impl Resolved<'_> {
    /// Stable owner-qualified key.
    pub fn key(self) -> DatasetKey {
        match self {
            Self::Local { dataset, .. } => dataset.key(),
            Self::External(e) => e.key(),
        }
    }
}
/// Immutable registry with exact indexes. Constructors only parse supplied identities.
#[derive(Clone, Debug)]
pub struct RegistrySnapshot {
    workspace: WorkspaceId,
    datasets: BTreeMap<DatasetId, LocalDataset>,
    external: BTreeMap<ExternalRegistrationId, ExternalRegistration>,
    names: BTreeMap<DatasetPath, Name>,
    raw_digest: ContentDigest,
    fingerprint: ContentDigest,
}
fn path(text: &str, scope: DatasetScope, location: &str) -> Result<DatasetPath> {
    if text.len() > 4096 || text.split('/').count() > 64 {
        return Err(error(RegistryErrorKind::Limit, location));
    }
    DatasetPath::parse_for_scope(text, scope)
        .map_err(|_| error(RegistryErrorKind::Invalid, location))
}
fn branch(text: &str, location: &str) -> Result<BranchName> {
    if text.len() > 4096 {
        return Err(error(RegistryErrorKind::Limit, location));
    }
    text.parse()
        .map_err(|_| error(RegistryErrorKind::Invalid, location))
}
fn claim<T: Ord>(map: &mut BTreeMap<T, String>, key: T, location: String) -> Result<()> {
    if let Some(previous) = map.get(&key) {
        let mut e = error(RegistryErrorKind::Collision, location);
        e.related = Some(previous.clone());
        return Err(e);
    }
    map.insert(key, location);
    Ok(())
}
impl RegistrySnapshot {
    /// Parse one format-1 registry in an explicitly supplied workspace context.
    pub fn parse(workspace: WorkspaceId, text: &str) -> Result<Self> {
        if text.len() > MAX_REGISTRY_BYTES {
            return Err(error(RegistryErrorKind::Limit, "/"));
        }
        let mut raw: Raw = toml::from_str(text).map_err(|e| {
            let mut err = error(RegistryErrorKind::Syntax, "/");
            err.span = e.span();
            err
        })?;
        if raw.format_version != 1 {
            return Err(error(RegistryErrorKind::Version, "/format_version"));
        }
        if raw.datasets.len()
            + raw.aliases.len()
            + raw.tombstones.len()
            + raw.external_registrations.len()
            > 100_000
        {
            return Err(error(RegistryErrorKind::Limit, "/"));
        }
        let mut datasets = BTreeMap::new();
        let mut external = BTreeMap::new();
        let mut names = BTreeMap::new();
        let mut claimed_names = BTreeMap::new();
        let mut claimed_ids = BTreeMap::new();
        for (section, records, tombstone) in [
            ("datasets", &raw.datasets, false),
            ("tombstones", &raw.tombstones, true),
        ] {
            for (index, r) in records.iter().enumerate() {
                let loc = format!("/{section}/{index}");
                let id: DatasetId =
                    r.id.parse()
                        .map_err(|_| error(RegistryErrorKind::Invalid, format!("{loc}/id")))?;
                let name = path(&r.path, DatasetScope::Local, &format!("{loc}/path"))?;
                claim(&mut claimed_ids, id, format!("{loc}/id"))?;
                claim(&mut claimed_names, name.clone(), format!("{loc}/path"))?;
                names.insert(
                    name.clone(),
                    if tombstone {
                        Name::Tombstone
                    } else {
                        Name::Local(id)
                    },
                );
                datasets.insert(
                    id,
                    LocalDataset {
                        key: DatasetKey::new(workspace, id),
                        path: name,
                        kind: r.kind,
                        tombstone,
                    },
                );
            }
        }
        for (index, r) in raw.aliases.iter().enumerate() {
            let loc = format!("/aliases/{index}");
            let id: DatasetId = r
                .target_id
                .parse()
                .map_err(|_| error(RegistryErrorKind::Invalid, format!("{loc}/target_id")))?;
            if !datasets.get(&id).is_some_and(|d| !d.tombstone) {
                return Err(error(
                    RegistryErrorKind::AliasTarget,
                    format!("{loc}/target_id"),
                ));
            }
            let name = path(&r.path, DatasetScope::Local, &format!("{loc}/path"))?;
            claim(&mut claimed_names, name.clone(), format!("{loc}/path"))?;
            names.insert(name, Name::Alias(id));
        }
        let mut registrations = BTreeMap::new();
        for (index, r) in raw.external_registrations.iter().enumerate() {
            let loc = format!("/external_registrations/{index}");
            let id: ExternalRegistrationId =
                r.id.parse()
                    .map_err(|_| error(RegistryErrorKind::Invalid, format!("{loc}/id")))?;
            claim(&mut registrations, id, format!("{loc}/id"))?;
            let provider: WorkspaceId = r.provider_workspace_id.parse().map_err(|_| {
                error(
                    RegistryErrorKind::Invalid,
                    format!("{loc}/provider_workspace_id"),
                )
            })?;
            if provider == workspace {
                return Err(error(
                    RegistryErrorKind::Invalid,
                    format!("{loc}/provider_workspace_id"),
                ));
            }
            let dataset: DatasetId = r.provider_dataset_id.parse().map_err(|_| {
                error(
                    RegistryErrorKind::Invalid,
                    format!("{loc}/provider_dataset_id"),
                )
            })?;
            let alias = path(&r.alias, DatasetScope::Foreign, &format!("{loc}/alias"))?;
            // external/<provider alias>/<dataset path> has two nonempty parts after external.
            if alias.as_str().split('/').count() < 3 {
                return Err(error(RegistryErrorKind::Invalid, format!("{loc}/alias")));
            }
            claim(&mut claimed_names, alias.clone(), format!("{loc}/alias"))?;
            let display_path = path(
                &r.provider_display_path,
                DatasetScope::Local,
                &format!("{loc}/provider_display_path"),
            )?;
            let default = branch(&r.default_branch, &format!("{loc}/default_branch"))?;
            if r.fallback_override.as_ref().is_some_and(|v| v.len() > 1000) {
                return Err(error(
                    RegistryErrorKind::Limit,
                    format!("{loc}/fallback_override"),
                ));
            }
            let fallback = r
                .fallback_override
                .as_ref()
                .map(|v| {
                    v.iter()
                        .enumerate()
                        .map(|(i, b)| branch(b, &format!("{loc}/fallback_override/{i}")))
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?;
            names.insert(alias.clone(), Name::External(id));
            external.insert(
                id,
                ExternalRegistration {
                    id,
                    alias,
                    key: DatasetKey::new(provider, dataset),
                    display_path,
                    branch: default,
                    fallback,
                },
            );
        }
        // Record ordering/formatting is presentation; fallback order remains semantic.
        raw.datasets.sort_by(|a, b| a.id.cmp(&b.id));
        raw.tombstones.sort_by(|a, b| a.id.cmp(&b.id));
        raw.aliases.sort_by(|a, b| a.path.cmp(&b.path));
        raw.external_registrations.sort_by(|a, b| a.id.cmp(&b.id));
        let fingerprint=content_digest(DigestKind::Catalog,&serde_json::json!({"registry_semantics_version":1,"workspace_id":workspace.to_string(),"registry":raw})).map_err(|_|error(RegistryErrorKind::Limit,"/"))?;
        let raw_digest = file_digest(&mut text.as_bytes())
            .map_err(|_| error(RegistryErrorKind::Invalid, "/"))?;
        Ok(Self {
            workspace,
            datasets,
            external,
            names,
            raw_digest,
            fingerprint,
        })
    }
    /// Explicit context; parsing never discovers a workspace through the current directory.
    pub fn workspace(&self) -> WorkspaceId {
        self.workspace
    }
    /// Byte digest for later expected-old/new registry replacement guards.
    pub fn raw_digest(&self) -> &ContentDigest {
        &self.raw_digest
    }
    /// Complete semantic registry fingerprint, including aliases/tombstones/provider policy.
    /// This is distinct from the narrower SDK CatalogSnapshotV1 wire projection.
    pub fn fingerprint(&self) -> &ContentDigest {
        &self.fingerprint
    }
    /// Read-only local records, in stable identity order, including tombstones.
    pub fn datasets(&self) -> impl Iterator<Item = &LocalDataset> {
        self.datasets.values()
    }
    /// Read-only foreign registrations in stable registration-ID order.
    pub fn external_registrations(&self) -> impl Iterator<Item = &ExternalRegistration> {
        self.external.values()
    }
    /// Exact registered local ID lookup; deleted identities never become a replacement.
    pub fn by_id(&self, id: DatasetId) -> Result<&LocalDataset> {
        let d = self
            .datasets
            .get(&id)
            .ok_or_else(|| error(RegistryErrorKind::Missing, "/reference"))?;
        if d.tombstone {
            return Err(error(RegistryErrorKind::Tombstone, "/reference"));
        }
        Ok(d)
    }
    /// Resolve an exact path or dataset:UUID. No fuzzy, relative or foreign-ID fallback.
    pub fn resolve(&self, reference: &str) -> Result<Resolved<'_>> {
        if reference.len() > 4096 {
            return Err(error(RegistryErrorKind::Limit, "/reference"));
        }
        if let Some(id) = reference.strip_prefix("dataset:") {
            let id = id
                .parse()
                .map_err(|_| error(RegistryErrorKind::Invalid, "/reference"))?;
            return Ok(Resolved::Local {
                dataset: self.by_id(id)?,
                via_alias: false,
            });
        }
        let name: DatasetPath = reference
            .parse()
            .map_err(|_| error(RegistryErrorKind::Invalid, "/reference"))?;
        match self.names.get(&name) {
            Some(Name::Local(id)) => Ok(Resolved::Local {
                dataset: self.by_id(*id)?,
                via_alias: false,
            }),
            Some(Name::Alias(id)) => Ok(Resolved::Local {
                dataset: self.by_id(*id)?,
                via_alias: true,
            }),
            Some(Name::Tombstone) => Err(error(RegistryErrorKind::Tombstone, "/reference")),
            Some(Name::External(id)) => self
                .external
                .get(id)
                .map(Resolved::External)
                .ok_or_else(|| error(RegistryErrorKind::Missing, "/reference")),
            None => {
                let prefix = format!("{reference}/");
                let namespace = self
                    .names
                    .range(name..)
                    .next()
                    .is_some_and(|(p, _)| p.as_str().starts_with(&prefix));
                Err(error(
                    if namespace {
                        RegistryErrorKind::Namespace
                    } else {
                        RegistryErrorKind::Missing
                    },
                    "/reference",
                ))
            }
        }
    }
    /// Validate an existing output reference; external boundaries are never destinations.
    pub fn resolve_output(&self, reference: &str) -> Result<&LocalDataset> {
        match self.resolve(reference)? {
            Resolved::Local { dataset, .. } => Ok(dataset),
            Resolved::External(_) => Err(error(RegistryErrorKind::ForeignOutput, "/reference")),
        }
    }
}
