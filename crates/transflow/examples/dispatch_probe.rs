//! Contributor-only entry point exercising real accepted-plan execution.
use serde_json::json;
use tf_catalog::workspace::Workspace;
use tf_exec::{
    ownership::{CoordinatorMode, RuntimeOwner},
    supervisor::Cancellation,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let [root, python, mode, target, settings] = args.as_slice() else {
        return Err("expected root python mode target settings-json".into());
    };
    let settings: serde_json::Value = serde_json::from_str(settings)?;
    let root = std::path::Path::new(root);
    let workspace = Workspace::load(root, Some(root))?;
    let owner = RuntimeOwner::acquire(root, workspace.config().id(), CoordinatorMode::Temporary)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result=rt.block_on(async {
        let request=transflow::build_plan::Request{python:Some(python.clone()),branch:settings["branch"].as_str().unwrap_or("feature").parse().map_err(|e|format!("{e}"))?,mode:match mode.as_str(){"full"=>tf_plan::scope::Mode::Full,"selected"=>tf_plan::scope::Mode::Selected,"between"=>tf_plan::scope::Mode::Between,_=>return Err("invalid mode".into())},targets:vec![target.clone()],boundaries:settings["boundaries"].as_array().map(|v|v.iter().filter_map(|v|v.as_str().map(str::to_owned)).collect()).unwrap_or_default(),exclusions:vec![],refresh_sources:vec![],pins:settings["pins"].as_array().map(|v|v.iter().map(|p|p.as_str().ok_or_else(||"invalid pin".to_owned())?.parse().map_err(|e|format!("{e}"))).collect::<Result<Vec<_>,String>>()).transpose()?.unwrap_or_default(),fallbacks:None,force:settings["force"]==true,parameters:json!({})};
        let c=transflow::build_plan::prepare(owner,request).await.map_err(|e|e.to_string())?;let plan=c.result.map_err(|e|e.to_string())?;
        let c=transflow::build_plan::accept(c.owner,plan.id.parse().map_err(|_|"bad id")?).await.map_err(|e|e.to_string())?;let accepted=c.result.map_err(|e|e.to_string())?;
        if settings["accept_only"] == true {
            return Ok(json!({"build":accepted.build.to_string(),"state":"QUEUED"}));
        }
        if let Some(code)=settings["replace_source"].as_str(){std::fs::write(root.join("src/items.py"),code).map_err(|e|e.to_string())?;}
        let cancel=Cancellation::default();
        if settings["cancel"]==true{cancel.cancel();}
        let c=transflow::dispatch::run_with_options(c.owner,accepted.build,cancel,transflow::dispatch::Options{progress:None,commands:None,providers:None,memory_estimates:settings["estimate"].as_u64().map(|n|accepted.plan.writes.iter().map(|w|Ok((w.job.parse().map_err(|_|"invalid job")?,n))).collect::<Result<_,String>>()).transpose()?.unwrap_or_default()}).await.map_err(|e|e.to_string())?;
        let r=c.result.map_err(|e|e.to_string())?;
        Ok::<_,String>(json!({"build":r.build.to_string(),"state":format!("{:?}",r.state),"jobs":r.jobs.iter().map(|j|json!({"job":j.id().to_string(),"dataset":j.target().dataset.dataset_id().to_string(),"state":j.state().name(),"attempts":j.attempts().iter().map(|a|a.id().to_string()).collect::<Vec<_>>()})).collect::<Vec<_>>()}))
    });
    println!(
        "{}",
        match result {
            Ok(r) => json!({"ok":true,"result":r}),
            Err(e) => json!({"ok":false,"error":e}),
        }
    );
    Ok(())
}
