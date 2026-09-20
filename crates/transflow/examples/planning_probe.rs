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
        if operation == "accept" {
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
                "full" => tf_plan::scope::Mode::Full,
                "selected" => tf_plan::scope::Mode::Selected,
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
                    force: false,
                    parameters: json!({}),
                },
            )
            .await
            .map_err(|e| e.to_string())?;
            completion
                .result
                .map(|plan| json!({"plan":plan}))
                .map_err(|e| e.to_string())
        }
    });
    match result {
        Ok(value) => println!("{}", json!({"ok":true,"result":value})),
        Err(error) => println!("{}", json!({"ok":false,"error":error})),
    };
    Ok(())
}
