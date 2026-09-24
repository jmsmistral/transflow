//! Explicit provenance expansion; foreign ancestors are never execution candidates.
use crate::{
    build_plan::{Error, failure, id},
    provider::{self, Request, Selection},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use tf_catalog::{
    RegistrySnapshot, candidate::CandidateIdentity, traversal::Traversal,
    validation::ValidatedGraph, workspace::Workspace,
};
use tf_domain::{DatasetKey, RequestId, VersionId, input::InputBindingKey};
use tf_exec::ownership::RuntimeOwner;

pub(crate) fn expand(
    owner: &mut RuntimeOwner,
    workspace: &Workspace,
    registry: &RegistrySnapshot,
    graph: &ValidatedGraph,
    traversal: &Traversal,
) -> Result<Value, Error> {
    let first = traversal.page(None, 100).map_err(failure)?;
    let mut foreign = BTreeMap::new();
    let mut selected_edges = BTreeSet::new();
    let mut cursor = None;
    loop {
        let page = traversal.page(cursor.as_deref(), 100).map_err(failure)?;
        for n in page.nodes {
            if n.node.external {
                foreign.insert(n.node.identity, n.depth);
            }
        }
        for e in page.edges {
            selected_edges.insert((e.consumer, e.alias));
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    let mut seeds = vec![];
    for definition in graph.candidate().definitions() {
        for input in &definition.inputs {
            if !foreign.contains_key(&input.identity)
                || !selected_edges.contains(&(definition.output.clone(), input.alias.clone()))
            {
                continue;
            }
            let consumer = match definition.output {
                CandidateIdentity::Registered(k) => k,
                CandidateIdentity::Pending(_) => DatasetKey::new(workspace.config().id(), id()?),
            };
            let (origin, selection) = crate::replicas::selection(
                registry,
                input,
                &InputBindingKey::new(consumer, input.alias.clone()).map_err(failure)?,
                &first.context.branch,
            )?;
            seeds.push((origin, selection, foreign[&input.identity]));
        }
    }
    if let CandidateIdentity::Registered(start) = first.request.start.clone()
        && start.workspace_id() != workspace.config().id()
    {
        for r in registry
            .external_registrations()
            .filter(|r| r.key() == start)
        {
            seeds.push((
                start,
                Selection {
                    consumer_workspace: workspace.config().id().to_string(),
                    consumer_dataset: start.dataset_id().to_string(),
                    alias: r.alias().as_str().into(),
                    dataset: start.dataset_id().to_string(),
                    output: first.context.branch.as_str().into(),
                    registered: r.default_branch().as_str().into(),
                    declared: json!({"kind":"omitted","name":null}),
                    stop: false,
                    role: "data".into(),
                    fallback: r
                        .fallback_override()
                        .map(|v| v.iter().map(ToString::to_string).collect()),
                },
                0,
            ));
        }
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(failure)?;
    let mut nodes = vec![];
    let mut edges = vec![];
    let mut queue = VecDeque::new();
    let mut visited = BTreeSet::new();
    let mut contexts = BTreeSet::new();
    let locator = |id| {
        workspace.local().provider(id).map(|p| {
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                workspace.root().join(p)
            }
        })
    };
    let mut metadata_bytes = 0usize;
    for (origin, selection, depth) in seeds {
        let context = serde_json::to_string(&selection).map_err(failure)?;
        if !contexts.insert((origin, context)) {
            continue;
        }
        let selected = (|| {
            let path = locator(origin.workspace_id())
                .ok_or_else(|| failure("provider locator unavailable"))?;
            let lease = id::<RequestId>()?.to_string();
            let operation = id::<RequestId>()?.to_string();
            let reply = provider::call(
                &path,
                origin.workspace_id(),
                Request::Resolve {
                    binding: Box::new(selection.clone()),
                    pin: None,
                    lease: lease.clone(),
                    operation: operation.clone(),
                },
            )?;
            provider::call(
                &path,
                origin.workspace_id(),
                Request::Release {
                    lease,
                    operation,
                    fence: 0,
                },
            )?;
            Ok::<_, Error>(reply["metadata"].clone())
        })();
        if let Ok(ref m) = selected {
            metadata_bytes =
                metadata_bytes.saturating_add(serde_json::to_vec(m).map_err(failure)?.len());
        }
        if metadata_bytes > 16 * 1024 * 1024 {
            return Err(failure(
                "Foreign lineage metadata exceeds 16 MiB; request a smaller --depth",
            ));
        }
        match selected {
            Ok(m)=>queue.push_back((origin,m["version"].as_str().ok_or_else(||failure("missing provenance version"))?.parse::<VersionId>().map_err(failure)?,depth,Some(m))),
            Err(_)=>nodes.push(json!({"workspace":origin.workspace_id().to_string(),"dataset":origin.dataset_id().to_string(),"version":null,"context":selection,"depth":depth.to_string(),"availability":"provider_unavailable","source_availability":"unavailable","executable":false})),
        }
    }
    while !queue.is_empty() {
        queue
            .make_contiguous()
            .sort_by_key(|(_, _, depth, _)| *depth);
        let Some((key, version, depth, seed)) = queue.pop_front() else {
            break;
        };
        let from_seed = seed.is_some();
        if !visited.insert((key, version)) {
            continue;
        }
        if visited.len() > 10000 || nodes.len() > 10000 || edges.len() > 100000 {
            return Err(failure(
                "Foreign provenance exceeds the supported bound; request a smaller --depth",
            ));
        }
        let metadata = match seed {
            Some(m) => Some(m),
            None => {
                let local = rt.block_on(async {
                    let mut owned = owner.open_store().await.map_err(failure)?;
                    let store = owned.repository().map_err(failure)?;
                    let m = if key.workspace_id() == workspace.config().id() {
                        store.export_metadata(key, version).await.ok()
                    } else {
                        store
                            .foreign_metadata(key, version)
                            .await
                            .map_err(failure)?
                    };
                    owned.close().await.map_err(failure)?;
                    Ok::<_, Error>(m)
                })?;
                match local {
                    Some(m) => Some(m),
                    None => locator(key.workspace_id()).and_then(|path| {
                        provider::call(
                            &path,
                            key.workspace_id(),
                            Request::Provenance {
                                dataset: key.dataset_id().to_string(),
                                version: version.to_string(),
                            },
                        )
                        .ok()
                    }),
                }
            }
        };
        let Some(m) = metadata else {
            nodes.push(json!({"workspace":key.workspace_id().to_string(),"dataset":key.dataset_id().to_string(),"version":version.to_string(),"depth":depth.to_string(),"availability":"metadata_unavailable","source_availability":"unavailable","executable":false}));
            continue;
        };
        if m["workspace"] != key.workspace_id().to_string()
            || m["dataset"] != key.dataset_id().to_string()
            || m["version"] != version.to_string()
        {
            return Err(failure("foreign provenance identity mismatch"));
        }
        if !from_seed {
            metadata_bytes =
                metadata_bytes.saturating_add(serde_json::to_vec(&m).map_err(failure)?.len());
        }
        if metadata_bytes > 16 * 1024 * 1024 {
            return Err(failure(
                "Foreign lineage metadata exceeds 16 MiB; request a smaller --depth",
            ));
        }
        let inputs = m["inputs"]
            .as_array()
            .ok_or_else(|| failure("foreign provenance inputs unavailable"))?;
        nodes.push(json!({"workspace":key.workspace_id().to_string(),"dataset":key.dataset_id().to_string(),"version":version.to_string(),"depth":depth.to_string(),"availability":"metadata_available","source":m["source"],"source_availability":"metadata_only","executable":false,"artifact":m["artifact"],"output_certificates":m["output_certificates"],"upstream_input_count":inputs.len().to_string(),"depth_limited":!inputs.is_empty() && first.request.depth.is_some_and(|max|depth>=max)}));
        if first.request.depth.is_some_and(|max| depth >= max) {
            continue;
        }
        for input in inputs {
            let parse = |field: &str| {
                input[field]
                    .as_str()
                    .ok_or_else(|| failure("invalid foreign provenance identity"))
            };
            let parent = DatasetKey::new(
                parse("workspace")?.parse().map_err(failure)?,
                parse("dataset")?.parse().map_err(failure)?,
            );
            let pv = parse("version")?.parse().map_err(failure)?;
            edges.push(json!({"consumer":{"workspace":key.workspace_id().to_string(),"dataset":key.dataset_id().to_string(),"version":version.to_string()},"input":input,"executable":false}));
            queue.push_back((parent, pv, depth + 1, None));
            if queue.len() > 100000 {
                return Err(failure(
                    "Foreign lineage frontier exceeds the supported bound; request a smaller --depth",
                ));
            }
        }
    }
    Ok(
        json!({"nodes":nodes,"edges":edges,"context":"provider_heads_then_exact_provenance","data_copied":false,"producer_execution":false}),
    )
}
