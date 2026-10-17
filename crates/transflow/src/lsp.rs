//! Read-only, workspace-qualified catalogue metadata. Never discovers/imports producers.

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tf_catalog::{
    RegistrySnapshot,
    workspace::{Workspace, read_authoring},
};
use tf_domain::{BranchName, DatasetId};
type AuthoringStamp = Vec<Option<(u64, u64, u64, i64, i64)>>;

struct Metadata {
    pub workspace: Workspace,
    pub registry: RegistrySnapshot,
    pub names: BTreeMap<String, Arc<Value>>,
    pub producers: BTreeMap<DatasetId, Value>,
    pub branch: BranchName,
    pub stale: bool,
    stamp: AuthoringStamp,
    follows_git: bool,
}
fn stamp(root: &Path) -> AuthoringStamp {
    use std::os::unix::fs::MetadataExt;
    [
        "workspace.toml",
        "transflow.local.toml",
        ".transflow/catalog.toml",
        ".transflow/runtime/graphs/current",
    ]
    .iter()
    .map(|name| {
        std::fs::symlink_metadata(root.join(name))
            .ok()
            .map(|m| (m.dev(), m.ino(), m.len(), m.ctime(), m.ctime_nsec()))
    })
    .collect()
}
impl Metadata {
    pub fn load(root: &Path, branch: Option<&str>) -> Result<Self, String> {
        let before = stamp(root);
        let workspace = Workspace::load(root, Some(root)).map_err(|e| e.to_string())?;
        let registry = RegistrySnapshot::parse(
            workspace.config().id(),
            &read_authoring(&workspace.root().join(".transflow/catalog.toml"))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let explicit = branch
            .map(str::parse::<BranchName>)
            .transpose()
            .map_err(|_| "Invalid editor branch")?;
        let git = if explicit.is_none() && workspace.config().follow_git_branch() {
            tf_catalog::git::inspect(&workspace).map_err(|e| e.to_string())?
        } else {
            None
        };
        let branch = tf_catalog::git::output_branch(
            workspace.config(),
            git.as_ref(),
            explicit.as_ref(),
            false,
        )
        .map_err(|e| e.to_string())?;
        let producers = crate::catalog::retained(&workspace).map_err(|e| e.to_string())?;
        let mut names = BTreeMap::new();
        for entry in tf_catalog::browse::entries(&registry) {
            let entry = Arc::new(entry);
            if entry["tombstone"] == true {
                continue;
            }
            let path = entry["path"]
                .as_str()
                .ok_or("Invalid catalogue path")?
                .to_owned();
            if entry["origin"] == "local" {
                let id = entry["dataset_id"]
                    .as_str()
                    .ok_or("Invalid identity")?
                    .parse()
                    .map_err(|_| "Invalid identity")?;
                for alias in registry.aliases(id) {
                    names.insert(alias.to_owned(), entry.clone());
                }
            }
            names.insert(path, entry);
        }
        let current_stamp = stamp(workspace.root());
        if before != current_stamp {
            return Err("Authoring metadata changed during refresh; retry".into());
        }
        let follows_git = explicit.is_none() && workspace.config().follow_git_branch();
        Ok(Self {
            workspace,
            registry,
            names,
            producers,
            branch,
            stale: false,
            stamp: current_stamp,
            follows_git,
        })
    }
    fn boxed(root: &Path, branch: Option<&str>) -> Result<Box<dyn tf_lsp::Catalogue>, String> {
        Ok(Box::new(Self::load(root, branch)?))
    }
}
impl tf_lsp::Catalogue for Metadata {
    fn needs_refresh(&self) -> bool {
        stamp(self.workspace.root()) != self.stamp
            || self.follows_git
                && match tf_catalog::git::inspect(&self.workspace) {
                    Ok(Some(git)) => git.branch() != Some(self.branch.as_str()),
                    Ok(None) => self.workspace.config().default_branch() != self.branch.as_str(),
                    Err(_) => true,
                }
    }
    fn resolves(&self, reference: &str) -> bool {
        self.registry.resolve(reference).is_ok()
    }
    fn names(&self) -> &BTreeMap<String, Arc<Value>> {
        &self.names
    }
    fn has_producers(&self) -> bool {
        !self.producers.is_empty()
    }
    fn stubs_available(&self) -> bool {
        self.workspace
            .root()
            .join(".transflow/runtime/generated/current/transflow/catalog.pyi")
            .is_file()
    }
    fn mark_stale(&mut self) {
        self.stale = true;
    }
    fn stale(&self) -> bool {
        self.stale
    }
    fn branch(&self) -> &str {
        self.branch.as_str()
    }
    fn contains(&self, path: &Path) -> bool {
        // Existing files, or new unsaved files beneath an existing canonical parent.
        let canonical = path.canonicalize().or_else(|_| {
            Ok::<_, std::io::Error>(
                path.parent()
                    .ok_or(std::io::ErrorKind::InvalidInput)?
                    .canonicalize()?
                    .join(path.file_name().ok_or(std::io::ErrorKind::InvalidInput)?),
            )
        });
        canonical.is_ok_and(|p| {
            self.workspace
                .roots()
                .iter()
                .any(|root| p.starts_with(root))
        })
    }
    fn location(&self, reference: &str) -> Option<Value> {
        let resolved = self.registry.resolve(reference).ok()?;
        if !matches!(resolved, tf_catalog::Resolved::Local { .. }) {
            return None;
        }
        let id = resolved.key().dataset_id();
        let location = self.producers.get(&id)?;
        let relative = Path::new(location["path"].as_str()?);
        if relative.is_absolute()
            || relative
                .components()
                .any(|p| !matches!(p, std::path::Component::Normal(_)))
        {
            return None;
        }
        let path = self.workspace.root().join(relative).canonicalize().ok()?;
        if !self
            .workspace
            .roots()
            .iter()
            .any(|root| path.starts_with(root))
        {
            return None;
        }
        let line = location["line"].as_u64()?.saturating_sub(1);
        Some(
            json!({"uri":tf_lsp::file_uri(&path)?,"range":{"start":{"line":line,"character":0},"end":{"line":line,"character":0}}}),
        )
    }
    fn hover(&self, reference: &str) -> Option<String> {
        let resolved = self.registry.resolve(reference).ok()?;
        let key = resolved.key();
        let entry = self.names.values().find(|e| {
            e["dataset_id"].as_str() == Some(&key.dataset_id().to_string())
                && e["workspace_id"].as_str() == Some(&key.workspace_id().to_string())
        })?;
        let mut text = format!(
            "{}\nDataset ID: {}\nWorkspace ID: {}\nSelected branch: {}\nCatalogue: {} (fingerprint {})\n",
            entry["path"].as_str()?,
            key.dataset_id(),
            key.workspace_id(),
            self.branch.as_str(),
            if self.stale {
                "last valid metadata; current catalogue/configuration invalid"
            } else {
                "working-tree metadata"
            },
            self.registry.fingerprint().hex()
        );
        if entry["origin"] != "local" {
            text.push_str("Producer: external provider not contacted; version/schema unknown.\n");
            return Some(text);
        }
        if let Some(p) = self.producers.get(&key.dataset_id()) {
            text.push_str(&format!("Producer: {}:{}\nDescriptor source snapshot: {} (captured metadata; current draft is not executed)\n",p["path"].as_str()?,p["line"],p["source_snapshot_id"].as_str()?));
        } else {
            text.push_str(
                "Producer: unavailable; run catalog sync explicitly to capture descriptors.\n",
            );
        }
        let db = self
            .workspace
            .root()
            .join(".transflow/runtime/catalog.sqlite");
        if !db.exists() {
            text.push_str("Built version/schema: unknown (no runtime metadata).\n");
            return Some(text);
        }
        let mut candidates = vec![self.branch.clone()];
        for branch in self.workspace.config().fallbacks(self.branch.as_str()) {
            if let Ok(branch) = branch.parse()
                && !candidates.contains(&branch)
            {
                candidates.push(branch);
            }
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        let head = runtime.block_on(async {
            let mut reader = tf_store::Reader::open_existing(&db).await?;
            let result = reader
                .api_head(self.workspace.config().id(), key.dataset_id(), &candidates)
                .await;
            reader.close().await?;
            result
        });
        match head {
            Ok(Some(head)) => {
                text.push_str(&format!("Retained built version: {}\nResolved data branch: {}\nBuilt source snapshot: {}\nAvailability: not verified; this is metadata only.\nKnown schema: {}\n",head["version"].as_str()?,head["resolved_branch"].as_str()?,head["source"].as_str()?,head["schema"]));
            }
            Ok(None) => text.push_str("Built version/schema: unknown (no head in selected branch or configured fallbacks).\n"),
            Err(_) => text.push_str("Built version/schema: unavailable (runtime metadata could not be read).\n"),
        }
        Some(text)
    }
    fn python(&self, explicit: Option<&PathBuf>) -> PathBuf {
        explicit
            .cloned()
            .or_else(|| self.workspace.local().interpreter().map(Path::to_path_buf))
            .unwrap_or_else(|| "python3.14".into())
    }
}

pub(crate) fn command() -> clap::Command {
    clap::Command::new("lsp")
        .disable_help_flag(true)
        .about("Serve dataset completion, hover, navigation and diagnostics over LSP stdio")
        .arg(
            clap::Arg::new("python")
                .long("python")
                .value_name("EXECUTABLE")
                .help("Explicit Python 3.14 syntax parser; never imports producers"),
        )
        .arg(
            clap::Arg::new("branch")
                .long("branch")
                .help("Explicit data branch for editor metadata"),
        )
}
pub(crate) fn execute(
    args: &clap::ArgMatches,
    root: Option<&String>,
    json: bool,
    out: &mut dyn std::io::Write,
    err: &mut dyn std::io::Write,
) -> std::io::Result<bool> {
    if json {
        writeln!(err, "LSP uses Content-Length JSON-RPC framing; omit --json")?;
        return Ok(false);
    }
    tf_lsp::serve(
        tf_lsp::Config {
            root: root.map(PathBuf::from),
            python: args.get_one::<String>("python").map(PathBuf::from),
            branch: args.get_one::<String>("branch").cloned(),
        },
        Metadata::boxed,
        out,
        err,
    )
}
