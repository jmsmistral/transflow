//! Context-bound metadata reads. No Python imports, provider contact or writer acquisition.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tf_api::{ApiError as E, Reply, Request};
use tf_catalog::{RegistrySnapshot, capture::SourceSnapshot, workspace::Workspace};
use tf_domain::{BranchName, WorkspaceId};
pub(crate) type Result<T> = std::result::Result<T, E>;
pub(crate) fn bad<T>(_: T) -> E {
    E::internal()
}
pub(crate) fn digest(v: &Value) -> Result<String> {
    let b = tf_protocol::canonical::canonical_json(v).map_err(bad)?;
    Ok(tf_protocol::canonical::file_digest(&mut b.as_slice())
        .map_err(bad)?
        .hex())
}
pub(crate) fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(bad)
}
pub(crate) async fn reader(root: &Path) -> Result<tf_store::Reader> {
    tf_store::Reader::open_existing(&root.join(".transflow/runtime/catalog.sqlite"))
        .await
        .map_err(bad)
}
pub(crate) struct Context {
    pub value: Value,
    pub fingerprint: String,
    pub registry: RegistrySnapshot,
    pub graph: Option<Value>,
    pub source: Option<tf_domain::SourceSnapshotId>,
    pub branch: BranchName,
    candidates: Vec<BranchName>,
    pub root: PathBuf,
}
impl Context {
    pub fn reply(&self, data: Value) -> Reply {
        Reply {
            data,
            context: Some(self.value.clone()),
            etag: Some(self.fingerprint.clone()),
        }
    }
    fn capture(&self) -> Result<SourceSnapshot> {
        SourceSnapshot::open(
            &self.root.join(".transflow/runtime/source-snapshots"),
            self.source.ok_or_else(E::missing)?,
        )
        .map_err(bad)
    }
}
/// View-local fallback tails are explicit; frozen plans/versions keep their own policy.
fn fallback_query(r: &Request) -> Result<Option<Vec<BranchName>>> {
    let Some(raw) = r.query.get("fallback") else {
        return Ok(None);
    };
    if r.query.contains_key("plan") || r.query.contains_key("version") {
        return Err(E::invalid());
    }
    let names: Vec<String> = serde_json::from_str(raw).map_err(|_| E::invalid())?;
    if names.len() > 100 {
        return Err(E::invalid());
    }
    names
        .into_iter()
        .map(|name| name.parse().map_err(|_| E::invalid()))
        .collect::<Result<Vec<_>>>()
        .map(Some)
}
pub(crate) fn context(root: &Path, wid: WorkspaceId, r: &Request) -> Result<Context> {
    let workspace = Workspace::load(root, Some(root)).map_err(bad)?;
    if workspace.config().id() != wid {
        return Err(E::conflict());
    }
    let branch: BranchName = r
        .query
        .get("branch")
        .ok_or_else(E::invalid)?
        .parse()
        .map_err(|_| E::invalid())?;
    let rt = runtime()?;
    let mut rd = rt.block_on(reader(root))?;
    let revision = rt.block_on(rd.api_revision()).map_err(bad)?;
    let mut fallback_override = fallback_query(r)?;
    let (mut registry, mut graph, mut source, mut selection) = if let Some(id) = r.query.get("plan")
    {
        let p = rt
            .block_on(rd.api_plan(id.parse().map_err(|_| E::invalid())?))
            .map_err(|_| E::missing())?;
        if p.workspace != wid.to_string() || p.output.name != branch.as_str() {
            return Err(E::conflict());
        }
        let source: tf_domain::SourceSnapshotId = p.source.parse().map_err(bad)?;
        fallback_override =
            serde_json::from_value::<Option<Vec<String>>>(p.context["fallback_override"].clone())
                .map_err(bad)?
                .map(|names| {
                    names
                        .into_iter()
                        .map(|s| s.parse().map_err(bad))
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?;
        (
            RegistrySnapshot::parse(wid, &p.replacement).map_err(bad)?,
            Some(
                json!({"discovery":p.discovery,"certificate":{"environment":p.environment["fingerprint"],"sdk":p.environment["runtime_version"]}}),
            ),
            Some(source),
            json!({"kind":"plan","id":p.id,"digest":p.digest().map_err(bad)?.hex()}),
        )
    } else {
        let bytes = tf_catalog::registry_write::RegistryFile::open(root)
            .and_then(|f| f.read())
            .map_err(bad)?;
        let registry =
            RegistrySnapshot::parse(wid, std::str::from_utf8(&bytes).map_err(bad)?).map_err(bad)?;
        let mut graph = tf_catalog::graph_cache::latest(root).map_err(bad)?;
        if graph
            .as_ref()
            .is_some_and(|g| g["certificate"]["registry_bytes"] != registry.raw_digest().hex())
        {
            graph = None;
        }
        let source = graph
            .as_ref()
            .map(|g| {
                g["source_snapshot_id"]
                    .as_str()
                    .ok_or_else(E::internal)?
                    .parse()
                    .map_err(bad)
            })
            .transpose()?;
        (
            registry,
            graph,
            source,
            json!({"kind":"retained_current","id":null,"digest":null}),
        )
    };
    if let Some(version) = r.query.get("version") {
        if r.query.contains_key("plan") {
            return Err(E::invalid());
        }
        let dataset: tf_domain::DatasetId = r
            .query
            .get("dataset")
            .ok_or_else(E::invalid)?
            .parse()
            .map_err(|_| E::invalid())?;
        let origin: WorkspaceId = r
            .query
            .get("origin_workspace")
            .map(|s| s.parse().map_err(|_| E::invalid()))
            .transpose()?
            .unwrap_or(wid);
        let version: tf_domain::VersionId = version.parse().map_err(|_| E::invalid())?;
        if !tf_catalog::browse::entries(&registry).iter().any(|e| {
            e["dataset_id"] == dataset.to_string() && e["workspace_id"] == origin.to_string()
        }) {
            return Err(E::missing());
        }
        let evidence = if origin == wid {
            let (id, plan) = rt
                .block_on(rd.api_version_source(origin, dataset, version))
                .map_err(|_| E::missing())?;
            source = Some(id);
            if let Some(p) = plan {
                registry = RegistrySnapshot::parse(wid, &p.replacement).map_err(bad)?;
                graph = Some(
                    json!({"discovery":p.discovery,"certificate":{"environment":p.environment["fingerprint"],"sdk":p.environment["runtime_version"]}}),
                );
            } else {
                let capture =
                    SourceSnapshot::open(&root.join(".transflow/runtime/source-snapshots"), id)
                        .map_err(bad)?;
                let bytes = capture
                    .read(Path::new(".transflow/catalog.toml"), 16 * 1024 * 1024)
                    .map_err(bad)?;
                registry = RegistrySnapshot::parse(wid, std::str::from_utf8(&bytes).map_err(bad)?)
                    .map_err(bad)?;
                graph = None;
            }
            json!({"source":id.to_string(),"dataset":dataset.to_string(),"origin":origin.to_string()})
        } else {
            let metadata = rt
                .block_on(rd.api_foreign_version(origin, dataset, version))
                .map_err(|_| E::missing())?;
            // Provider source is not stored in this workspace. Keep the distinction explicit.
            source = None;
            graph = None;
            json!({"metadata":metadata,"dataset":dataset.to_string(),"origin":origin.to_string()})
        };
        selection = json!({"kind":if origin==wid{"local_version"}else{"foreign_version"},"id":version.to_string(),"digest":digest(&evidence)?});
    } else if r.query.contains_key("dataset") {
        return Err(E::invalid());
    }
    rt.block_on(rd.close()).map_err(bad)?;
    let config = tf_catalog::workspace::read_authoring(&root.join("workspace.toml"))
        .map_err(bad)?
        .into_bytes();
    let policy_config = if let Some(id) = source {
        let capture = SourceSnapshot::open(&root.join(".transflow/runtime/source-snapshots"), id)
            .map_err(bad)?;
        let bytes = capture
            .read(Path::new("workspace.toml"), 16 * 1024 * 1024)
            .map_err(bad)?;
        tf_catalog::workspace::WorkspaceConfig::parse(std::str::from_utf8(&bytes).map_err(bad)?)
            .map_err(bad)?
    } else {
        tf_catalog::workspace::WorkspaceConfig::parse(std::str::from_utf8(&config).map_err(bad)?)
            .map_err(bad)?
    };
    let policy = policy_config
        .input_policies()
        .map_err(bad)?
        .local(
            &branch,
            tf_domain::BranchSelector::Current,
            tf_domain::FallbackPermission::Allowed,
            fallback_override.as_deref(),
        )
        .map_err(bad)?;
    let candidates = policy.candidates().to_vec();
    let mut value = json!({"workspace":wid.to_string(),"branch":branch.as_str(),"source":source.map(|s|s.to_string()),"registry":registry.raw_digest().hex(),"runtime_revision":revision,"configuration":tf_protocol::canonical::file_digest(&mut config.as_slice()).map_err(bad)?.hex(),"selection":selection,"graph":graph.as_ref().map(digest).transpose()?,"freshness":"unknown"});
    value["fallback_policy"] = json!(candidates.iter().map(|b| b.as_str()).collect::<Vec<_>>());
    let fingerprint = digest(&value)?;
    value["fingerprint"] = fingerprint.clone().into();
    if r.query.get("context").is_some_and(|s| s != &fingerprint) {
        return Err(E::conflict());
    }
    Ok(Context {
        value,
        fingerprint,
        registry,
        graph,
        source,
        branch,
        candidates,
        root: root.to_owned(),
    })
}
pub(crate) fn keys(r: &Request, extra: &[&str]) -> Result<()> {
    if r.query.keys().any(|k| {
        !matches!(
            k.as_str(),
            "branch" | "fallback" | "plan" | "context" | "version" | "dataset" | "origin_workspace"
        ) && !extra.contains(&k.as_str())
    }) {
        return Err(E::invalid());
    }
    Ok(())
}
pub(crate) fn limit(r: &Request, default: usize, max: usize) -> Result<usize> {
    let n = r
        .query
        .get("limit")
        .map(|s| s.parse::<usize>().map_err(|_| E::invalid()))
        .transpose()?
        .unwrap_or(default);
    if n == 0 || n > max {
        return Err(E::invalid());
    }
    Ok(n)
}
pub(crate) fn cursor(r: &Request, c: &Context) -> Result<(String, String)> {
    let mut q = r.query.clone();
    q.remove("cursor");
    q.remove("limit");
    q.remove("context");
    let binding = digest(&json!([c.fingerprint, r.path, q]))?;
    let after = if let Some(cursor) = r.query.get("cursor") {
        let (fp, after) = cursor.split_once(':').ok_or_else(E::invalid)?;
        if fp != binding {
            return Err(E::conflict());
        }
        after.to_owned()
    } else {
        String::new()
    };
    Ok((binding, after))
}
pub(crate) fn read(root: &Path, wid: WorkspaceId, r: &Request) -> Result<Reply> {
    let c = context(root, wid, r)?;
    let path = r.path.strip_prefix("/api/v1/").ok_or_else(E::missing)?;
    let data = if crate::api_history::handles(path) {
        crate::api_history::read(root, wid, r, &c, path)?
    } else if path == "context" {
        keys(r, &[])?;
        c.value.clone()
    } else if path == "datasets" || path == "externals" {
        keys(
            r,
            &[
                "limit",
                "cursor",
                "filter",
                "fuzzy",
                "origin",
                "id",
                "source_path",
                "tag",
                "column",
            ],
        )?;
        let n = limit(r, 50, 200)?;
        let (binding, after) = cursor(r, &c)?;
        let all = entries(&c, r)?
            .into_iter()
            .filter(|v| {
                r.query
                    .get("filter")
                    .is_none_or(|s| v["path"].as_str().is_some_and(|p| p.contains(s)))
            })
            .collect::<Vec<_>>();
        let offset = if after.is_empty() {
            0
        } else {
            all.iter()
                .position(|v| v["path"] == after)
                .ok_or_else(E::invalid)?
                + 1
        };
        if offset > all.len() {
            return Err(E::invalid());
        }
        let end = offset.saturating_add(n).min(all.len());
        json!({"entries":all.get(offset..end).ok_or_else(E::invalid)?,"total":all.len().to_string(),"next_cursor":(end<all.len()).then(||format!("{binding}:{}",all.get(end-1).and_then(|v|v["path"].as_str()).unwrap_or_default())),"schema":null,"freshness":"unknown"})
    } else if path.starts_with("datasets/") && path.ends_with("/lineage") {
        let id = path
            .trim_start_matches("datasets/")
            .trim_end_matches("/lineage");
        let id: tf_domain::DatasetId = id.parse().map_err(|_| E::invalid())?;
        let origin = r
            .query
            .get("origin_workspace")
            .cloned()
            .unwrap_or_else(|| wid.to_string());
        let entry = tf_catalog::browse::entries(&c.registry)
            .into_iter()
            .find(|e| e["dataset_id"] == id.to_string() && e["workspace_id"] == origin)
            .ok_or_else(E::missing)?;
        let mut query = r.clone();
        query.query.insert(
            "start".into(),
            entry["path"].as_str().ok_or_else(E::internal)?.into(),
        );
        lineage(&c, &query)?
    } else if path == "branches" || path == "builds" {
        keys(
            r,
            if path == "branches" {
                &["limit", "cursor", "datasets"]
            } else {
                &["limit", "cursor"]
            },
        )?;
        let n = limit(r, 50, 200)?;
        let (binding, after) = cursor(r, &c)?;
        if !after.is_empty() {
            let _: tf_domain::RequestId = after.parse().map_err(|_| E::invalid())?;
        }
        let rt = runtime()?;
        let mut rd = rt.block_on(reader(root))?;
        let mut entries = if path == "branches" {
            let datasets: Option<Vec<tf_domain::DatasetId>> = r
                .query
                .get("datasets")
                .map(|raw| {
                    let names: Vec<String> = serde_json::from_str(raw).map_err(|_| E::invalid())?;
                    names
                        .into_iter()
                        .map(|name| name.parse().map_err(|_| E::invalid()))
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?;
            rt.block_on(rd.api_branches(wid, &after, n as u32 + 1, datasets.as_deref()))
                .map_err(bad)?
        } else {
            rt.block_on(rd.api_builds(wid, &c.branch, &after, n as u32 + 1))
                .map_err(bad)?
        };
        rt.block_on(rd.close()).map_err(bad)?;
        let more = entries.len() > n;
        entries.truncate(n);
        let next = if more {
            entries
                .last()
                .and_then(|e| e["id"].as_str())
                .map(|s| format!("{binding}:{s}"))
        } else {
            None
        };
        json!({"entries":entries,"next_cursor":next})
    } else if let Some(rest) = path.strip_prefix("datasets/") {
        keys(r, &["limit", "cursor", "origin_workspace"])?;
        let (id, versions) = rest
            .strip_suffix("/versions")
            .map(|s| (s, true))
            .unwrap_or((rest, false));
        let id: tf_domain::DatasetId = id.parse().map_err(|_| E::invalid())?;
        let origin: WorkspaceId = r
            .query
            .get("origin_workspace")
            .map(|s| s.parse().map_err(|_| E::invalid()))
            .transpose()?
            .unwrap_or(wid);
        let entry = tf_catalog::browse::entries(&c.registry)
            .into_iter()
            .find(|e| e["dataset_id"] == id.to_string() && e["workspace_id"] == origin.to_string())
            .ok_or_else(E::missing)?;
        if !versions {
            let mut entries = enrich(&c, vec![entry])?;
            entries.pop().ok_or_else(E::missing)?
        } else {
            let n = limit(r, 50, 200)?;
            let (binding, after) = cursor(r, &c)?;
            if !after.is_empty() {
                let _: tf_domain::VersionId = after.parse().map_err(|_| E::invalid())?;
            }
            let rt = runtime()?;
            let mut rd = rt.block_on(reader(root))?;
            let mut entries = rt
                .block_on(rd.api_versions(origin, id, &after, n as u32 + 1, origin != wid))
                .map_err(bad)?;
            rt.block_on(rd.close()).map_err(bad)?;
            let more = entries.len() > n;
            entries.truncate(n);
            let next = if more {
                entries
                    .last()
                    .and_then(|e| e["version"].as_str())
                    .map(|s| format!("{binding}:{s}"))
            } else {
                None
            };
            json!({"entries":entries,"next_cursor":next})
        }
    } else if let Some(id) = path.strip_prefix("plans/") {
        keys(r, &[])?;
        let rt = runtime()?;
        let mut rd = rt.block_on(reader(root))?;
        let p = rt
            .block_on(rd.api_plan(id.parse().map_err(|_| E::invalid())?))
            .map_err(|_| E::missing())?;
        rt.block_on(rd.close()).map_err(bad)?;
        if r.query.get("plan") != Some(&p.id) {
            return Err(E::conflict());
        }
        crate::inspection_cli::plan_value("plan", &p).map_err(bad)?
    } else if let Some(id) = path.strip_prefix("source/") {
        keys(r, &["path", "offset", "limit"])?;
        if Some(id) != c.source.as_ref().map(|s| s.to_string()).as_deref() {
            return Err(E::conflict());
        }
        let path = r.query.get("path").ok_or_else(E::invalid)?;
        let content = c
            .capture()?
            .read(Path::new(path), 16 * 1024 * 1024)
            .map_err(|_| E::missing())?;
        let text = std::str::from_utf8(&content).map_err(|_| E::invalid())?;
        let offset = r
            .query
            .get("offset")
            .map(|v| v.parse::<usize>().map_err(|_| E::invalid()))
            .transpose()?
            .unwrap_or(0);
        let n = limit(r, 200, 1000)?;
        let lines = text.lines().skip(offset).take(n + 1).collect::<Vec<_>>();
        let excerpt = lines.iter().take(n).copied().collect::<Vec<_>>().join("\n");
        if excerpt.len() > 64 * 1024 {
            return Err(E::new(
                413,
                "TF_API_SOURCE_LIMIT",
                "Request a smaller source range",
            ));
        }
        json!({"source":id,"path":path,"offset":offset,"text":excerpt,"next_offset":(lines.len()>n).then(||offset+n)})
    } else if path == "lineage" {
        lineage(&c, r)?
    } else if let Some(id) = path.strip_prefix("builds/") {
        keys(r, &[])?;
        let rt = runtime()?;
        let mut rd = rt.block_on(reader(root))?;
        let v = rt
            .block_on(rd.build_report(wid, id.parse().map_err(|_| E::invalid())?))
            .map_err(|_| E::missing())?;
        rt.block_on(rd.close()).map_err(bad)?;
        if v["plan"]["output"]["name"] != c.branch.as_str()
            || c.source.is_some_and(|id| v["source"] != id.to_string())
        {
            return Err(E::conflict());
        }
        v
    } else {
        return Err(E::missing());
    };
    // Fence a publication or authoring edit that raced the read; never label a mixed response.
    if context(root, wid, r)?.fingerprint != c.fingerprint {
        return Err(E::conflict());
    }
    Ok(c.reply(data))
}
fn lineage(c: &Context, r: &Request) -> Result<Value> {
    use tf_catalog::{
        traversal::{self, Graph},
        validation::{self, ValidationRequest},
    };
    keys(
        r,
        &[
            "start",
            "end",
            "direction",
            "depth",
            "limit",
            "cursor",
            "expand",
            "connections",
            "lookup",
            "incoming",
        ],
    )?;
    let g = c.graph.as_ref().ok_or_else(E::missing)?;
    let capture = c.capture()?;
    let config = String::from_utf8(
        capture
            .read(Path::new("workspace.toml"), 16 * 1024 * 1024)
            .map_err(bad)?,
    )
    .map_err(bad)?;
    let modules = serde_json::from_value(g["discovery"]["module_index"].clone()).map_err(bad)?;
    let request = ValidationRequest {
        registry: &c.registry,
        source: c.source.ok_or_else(E::missing)?,
        source_digest: capture.digest(),
        config_toml: &config,
        modules: &modules,
        discovery: &g["discovery"],
        environment_fingerprint: g["certificate"]["environment"]
            .as_str()
            .ok_or_else(E::internal)?,
        sdk_version: g["certificate"]["sdk"].as_str().ok_or_else(E::internal)?,
        check_semantics: validation::CHECK_SEMANTICS,
    };
    let validated = validation::validate(&request).map_err(bad)?;
    let graph = Graph::validated(&validated, &request, c.branch.clone()).map_err(bad)?;
    let direction = match r
        .query
        .get("direction")
        .map(String::as_str)
        .unwrap_or("upstream")
    {
        "upstream" => traversal::Direction::Upstream,
        "downstream" => traversal::Direction::Downstream,
        _ => return Err(E::invalid()),
    };
    let incoming = match r.query.get("incoming").map(String::as_str) {
        None => false,
        Some("true") => true,
        _ => return Err(E::invalid()),
    };
    let traversal = if let Some(paths) = r.query.get("lookup") {
        if [
            "start",
            "end",
            "depth",
            "direction",
            "connections",
            "incoming",
        ]
        .iter()
        .any(|k| r.query.contains_key(*k))
        {
            return Err(E::invalid());
        }
        let paths: Vec<String> = serde_json::from_str(paths).map_err(|_| E::invalid())?;
        if paths.is_empty() || paths.len() > 100 {
            return Err(E::invalid());
        }
        let ids = paths
            .iter()
            .map(|p| graph.resolve(p).map_err(|_| E::missing()))
            .collect::<Result<std::collections::BTreeSet<_>>>()?;
        graph.lookup(&ids).map_err(bad)?
    } else if let Some(consumers) = r.query.get("connections") {
        if ["start", "end", "depth", "direction", "incoming"]
            .iter()
            .any(|k| r.query.contains_key(*k))
        {
            return Err(E::invalid());
        }
        let paths: Vec<String> = serde_json::from_str(consumers).map_err(|_| E::invalid())?;
        if paths.is_empty() || paths.len() > 100 {
            return Err(E::invalid());
        }
        let ids = paths
            .iter()
            .map(|p| graph.resolve(p).map_err(|_| E::missing()))
            .collect::<Result<std::collections::BTreeSet<_>>>()?;
        graph.connections(&ids).map_err(bad)?
    } else if let Some(end) = r.query.get("end") {
        if r.query.contains_key("depth") || direction != traversal::Direction::Downstream {
            return Err(E::invalid());
        }
        graph
            .paths(
                graph
                    .resolve(r.query.get("start").ok_or_else(E::invalid)?)
                    .map_err(|_| E::missing())?,
                graph.resolve(end).map_err(|_| E::missing())?,
            )
            .map_err(bad)?
    } else {
        graph
            .traverse(traversal::Request {
                start: graph
                    .resolve(r.query.get("start").ok_or_else(E::invalid)?)
                    .map_err(|_| E::missing())?,
                direction,
                depth: r
                    .query
                    .get("depth")
                    .map(|s| traversal::parse_depth(s).map_err(|_| E::invalid()))
                    .transpose()?,
            })
            .map_err(bad)?
    };
    let traversal = if incoming {
        graph.with_incoming(traversal).map_err(bad)?
    } else {
        traversal
    };
    let n = limit(r, 100, 500)?;
    let first = traversal.page(None, n).map_err(bad)?;
    if first.total_nodes > 500 && r.query.get("expand").map(String::as_str) != Some("true") {
        return Err(E::new(
            409,
            "TF_API_GRAPH_EXPANSION",
            "Explicit expansion is required for graphs above 500 nodes",
        ));
    }
    let (binding, after) = cursor(r, c)?;
    let p = traversal
        .page((!after.is_empty()).then_some(after.as_str()), n)
        .map_err(|_| E::conflict())?;
    let rt = runtime()?;
    let mut reader = rt.block_on(reader(&c.root))?;
    let mut nodes = Vec::with_capacity(p.nodes.len());
    for n in &p.nodes {
        // Never infer absence from unavailable foreign or historical metadata.
        let publication = if n.node.external || c.value["selection"]["kind"] != "retained_current" {
            "unknown"
        } else if let tf_catalog::candidate::CandidateIdentity::Registered(key) = &n.node.identity {
            if rt
                .block_on(reader.api_head(key.workspace_id(), key.dataset_id(), &c.candidates))
                .map_err(bad)?
                .is_some()
            {
                "published"
            } else {
                "missing"
            }
        } else {
            "missing"
        };
        let resource_type = if n.node.external {
            "external"
        } else if let Some(def) = validated
            .candidate()
            .definitions()
            .iter()
            .find(|d| d.output == n.node.identity)
        {
            match def.declaration["engine"].as_str() {
                Some("polars") => "polars_transform",
                Some("sql") | Some("duckdb") => "sql_transform",
                _ => "unknown",
            }
        } else if n.node.producer {
            "unknown"
        } else {
            "dataset"
        };
        nodes.push(json!({"identity":crate::why::identity(&n.node.identity),"paths":n.node.paths.iter().map(|p|p.as_str()).collect::<Vec<_>>(),"depth":n.depth.to_string(),"external":n.node.external,"producer":n.node.producer,"parent_count":graph.neighbour_count(&n.node.identity,traversal::Direction::Upstream).to_string(),"child_count":graph.neighbour_count(&n.node.identity,traversal::Direction::Downstream).to_string(),"publication":publication,"resource_type":resource_type}));
    }
    let edges=p.edges.iter().map(|e|{let branch=match &e.declared_branch{tf_domain::BranchSelector::Omitted=>json!({"kind":"omitted","name":null}),tf_domain::BranchSelector::Current=>json!({"kind":"current","name":null}),tf_domain::BranchSelector::Named(n)=>json!({"kind":"named","name":n.as_str()})};json!({"parent":crate::why::identity(&e.parent),"consumer":crate::why::identity(&e.consumer),"alias":e.alias,"role":e.role.name(),"declared_branch":branch,"stop_branch_fallback":e.stop_branch_fallback,"checks":e.checks})}).collect::<Vec<_>>();
    Ok(
        json!({"nodes":nodes,"edges":edges,"next_cursor":p.next_cursor.map(|s|format!("{binding}:{s}")),"total_nodes":p.total_nodes,"total_edges":p.total_edges,"omitted_nodes":p.omitted_nodes,"omitted_edges":p.omitted_edges,"remaining_nodes":p.remaining_nodes,"remaining_edges":p.remaining_edges,"scope_complete":p.scope_complete,"external_expanded":false}),
    )
}

fn enrich(c: &Context, mut entries: Vec<Value>) -> Result<Vec<Value>> {
    let candidate = c
        .graph
        .as_ref()
        .map(|g| {
            tf_catalog::candidate::CandidateCatalog::prepare(
                &c.registry,
                c.source.ok_or_else(E::missing)?,
                &g["discovery"],
            )
            .map_err(bad)
        })
        .transpose()?;
    let rt = runtime()?;
    let mut reader = rt.block_on(reader(&c.root))?;
    let mut encoded_bytes = 0_usize;
    for e in &mut entries {
        e["producer"] = Value::Null;
        e["tags"] = Value::Null;
        e["head"] = Value::Null;
        e["freshness"] = "unknown".into();
        if e["origin"] == "local" {
            let id: tf_domain::DatasetId = e["dataset_id"]
                .as_str()
                .ok_or_else(E::internal)?
                .parse()
                .map_err(bad)?;
            let wid: WorkspaceId = e["workspace_id"]
                .as_str()
                .ok_or_else(E::internal)?
                .parse()
                .map_err(bad)?;
            // Historical definition browsing must not label today's head as that historical version.
            if matches!(
                c.value["selection"]["kind"].as_str(),
                Some("retained_current")
            ) {
                e["head"] = rt
                    .block_on(reader.api_head(wid, id, &c.candidates))
                    .map_err(bad)?
                    .unwrap_or(Value::Null);
            }
            if let Some(def) = candidate.as_ref().and_then(|cat| {
                cat.definitions()
                    .iter()
                    .find(|d| d.path.as_str() == e["path"])
            }) {
                e["producer"] = json!({"path":def.location.path,"line":def.location.line,"source":c.source.map(|id|id.to_string()),"retained":true});
                e["tags"] = def.declaration["tags"].clone();
            }
        }
        encoded_bytes = encoded_bytes.saturating_add(serde_json::to_vec(e).map_err(bad)?.len());
        if encoded_bytes > 16 * 1024 * 1024 {
            return Err(E::new(
                413,
                "TF_API_CATALOG_LIMIT",
                "Narrow the catalogue path or identity filter",
            ));
        }
    }
    rt.block_on(reader.close()).map_err(bad)?;
    Ok(entries)
}
fn fuzzy_match(path: &str, query: &str) -> bool {
    let lowered = path.to_lowercase();
    let mut chars = lowered.chars();
    query
        .to_lowercase()
        .chars()
        .all(|wanted| chars.any(|c| c == wanted))
}
fn entries(c: &Context, r: &Request) -> Result<Vec<Value>> {
    let raw = tf_catalog::browse::entries(&c.registry)
        .into_iter()
        .filter(|e| {
            (r.path != "/api/v1/externals" || e["origin"] == "external")
                && r.query.get("origin").is_none_or(|v| e["origin"] == *v)
                && r.query.get("id").is_none_or(|v| e["dataset_id"] == *v)
                && r.query
                    .get("fuzzy")
                    .is_none_or(|v| e["path"].as_str().is_some_and(|p| fuzzy_match(p, v)))
                && r.query
                    .get("filter")
                    .is_none_or(|v| e["path"].as_str().is_some_and(|p| p.contains(v)))
        })
        .collect();
    let all = enrich(c, raw)?;
    Ok(all
        .into_iter()
        .filter(|e| {
            r.query.get("source_path").is_none_or(|v| {
                e["producer"]["path"]
                    .as_str()
                    .is_some_and(|s| s.contains(v))
            }) && r.query.get("tag").is_none_or(|v| {
                e["tags"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|s| s == v))
            }) && r.query.get("column").is_none_or(|v| {
                e["head"]["schema"]["fields"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|f| f["name"] == *v))
            })
        })
        .collect())
}
