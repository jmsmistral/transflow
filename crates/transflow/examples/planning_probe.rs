//! Contributor-only entry point for cross-language preparation tests; not an installed CLI.
use serde_json::json;
use tf_catalog::workspace::Workspace;
use tf_exec::ownership::{CoordinatorMode, RuntimeOwner};
use transflow::build_plan::{self, Request};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, python, operation, reference, ..] = args.as_slice() else {
        return Err("expected root, python, operation, reference".into());
    };
    let root = std::path::Path::new(root);
    let workspace = Workspace::load(root, Some(root))?;
    let owner = RuntimeOwner::acquire(root, workspace.config().id(), CoordinatorMode::Temporary)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        if operation == "why" {
            let now=i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_micros()).map_err(|e|e.to_string())?;
            let completion=transflow::why::inspect(owner,transflow::why::Request{python:Some(python.clone()),branch:"feature".parse().map_err(|_|"invalid branch")?,fallbacks:None,semantics:Default::default(),at_us:now}).await.map_err(|e|e.to_string())?;
            completion.result.map(|r|json!({"source":r.source,"branch":r.branch.as_str(),"datasets":r.datasets.values().map(|s|json!({"materialization":format!("{:?}",s.materialization),"direct_data":format!("{:?}",s.direct_data),"direct_logic":format!("{:?}",s.direct_logic),"reasons":s.reasons.iter().map(|r|json!({"code":r.code,"human":r.human(&Default::default())})).collect::<Vec<_>>()})).collect::<Vec<_>>()})).map_err(|e|e.to_string())
        } else if operation == "accept" {
            let completion =
                build_plan::accept(owner, reference.parse().map_err(|_| "invalid plan ID")?)
                    .await
                    .map_err(|e| e.to_string())?;
            completion
                .result
                .map(|r| json!({"build":r.build.to_string(),"plan":r.plan}))
                .map_err(|e| e.to_string())
        } else {
            let mode = match operation.as_str() {
                "full" | "cache-full" => tf_plan::scope::Mode::Full,
                "selected" | "cache" | "cache-force" => tf_plan::scope::Mode::Selected,
                "between" => tf_plan::scope::Mode::Between,
                _ => return Err("invalid probe mode".into()),
            };
            let completion = build_plan::prepare(
                owner,
                Request {
                    python: Some(python.clone()),
                    branch: "feature".parse().map_err(|_| "invalid branch")?,
                    mode,
                    targets: vec![reference.clone()],
                    boundaries: args.get(4).map(|r| vec![r.clone()]).unwrap_or_default(),
                    exclusions: vec![],
                    refresh_sources: vec![],
                    pins: vec![],
                    fallbacks: None,
                    force: operation == "cache-force",
                    parameters: json!({}),
                },
            )
            .await
            .map_err(|e| e.to_string())?;
            if operation.starts_with("cache") {
                let plan = completion.result.map_err(|e| e.to_string())?;
                let accepted = build_plan::accept(completion.owner, plan.id.parse().map_err(|_| "invalid plan ID")?).await.map_err(|e|e.to_string())?;
                let value = accepted.result.map_err(|e|e.to_string())?;
                let job = value.plan.writes.last().ok_or("missing job")?.job.parse().map_err(|_|"invalid job")?;
                let now = i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_micros()).map_err(|e|e.to_string())?;
                let prepared = transflow::cache::prepare(accepted.owner, value.build, job, transflow::cache::Semantics { writer: json!({"normalization":"v1"}), evaluation_us: None, secret_versions: Default::default(), checks: vec![] }, now).await.map_err(|e|e.to_string())?;
                let result = prepared.result.map_err(|e|e.to_string())?;
                match result {
                    transflow::cache::Decision::Pending => Ok(json!({"decision":"pending"})),
                    transflow::cache::Decision::Execute { reason, request } => Ok(json!({"decision":"execute","reason":reason,"compute":request.contract.compute_fingerprint})),
                    transflow::cache::Decision::Ready(request) => {
                        let key = request.contract.compute_fingerprint.clone();
                        let reused = tf_exec::cache::reuse(prepared.owner, *request, tf_domain::RequestId::from_bytes(*job.as_bytes()), move || Ok(now)).await.map_err(|e|e.to_string())?;
                        let result = reused.result.map_err(|e|e.to_string())?;
                        Ok(json!({"decision":"ready","compute":key,"hit":result.is_some()}))
                    }
                }
            } else {
                completion.result.map(|plan| json!({"plan":plan})).map_err(|e|e.to_string())
            }
        }
    });
    match result {
        Ok(value) => println!("{}", json!({"ok":true,"result":value})),
        Err(error) => println!("{}", json!({"ok":false,"error":error})),
    };
    Ok(())
}
