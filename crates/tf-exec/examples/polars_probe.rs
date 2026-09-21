//! Native adapter qualification only: synthetic publication fixture and isolated wheel worker.
use serde_json::{Value, json};
use std::{
    error::Error,
    fs::{self, File},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    time::Duration,
};
use tf_domain::RequestId;
use tf_exec::{
    ownership::{CoordinatorMode, RuntimeOwner},
    supervisor::{Cancellation, Launch, Policy},
};
use tf_protocol::{
    Operation,
    canonical::{ContentDigest, DigestKind},
};
use tf_store::artifacts::ArtifactStore;
#[path = "../tests/support/publication.rs"]
mod fixture;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mode = args.first().ok_or("Expected mode")?;
    let root = PathBuf::from(args.get(1).ok_or("Expected workspace")?);
    fs::create_dir_all(root.join(".transflow/runtime"))?;
    fs::set_permissions(
        root.join(".transflow/runtime"),
        fs::Permissions::from_mode(0o700),
    )?;
    let root = root.canonicalize()?;
    if mode == "stage" {
        let _owner =
            RuntimeOwner::acquire(&root, fixture::workspace(), CoordinatorMode::Temporary)?;
        let source = PathBuf::from(args.get(2).ok_or("Expected source directory")?);
        let names: Vec<PathBuf> = args.iter().skip(3).map(PathBuf::from).collect();
        let store = ArtifactStore::open(&root)?;
        let candidate = store.prepare(
            &File::open(source)?,
            &names,
            fixture::writer(),
            tf_exec::discovery::random_id()?.parse::<RequestId>()?,
            |_| Ok(()),
        )?;
        let object = candidate.install(|_| Ok(()))?;
        let hex = object.digest().hex();
        println!(
            "{}",
            json!({"manifest":object.manifest(), "artifact_digest":hex,
            "artifact_root":root.join(".transflow/runtime/objects").join(&hex[..2]).join(&hex)})
        );
        return Ok(());
    }
    if mode != "execute" {
        return Err("Expected stage or execute".into());
    }
    let python = PathBuf::from(args.get(2).ok_or("Expected worker launcher")?);
    let request: Value =
        serde_json::from_reader(File::open(args.get(3).ok_or("Expected request")?)?)?;
    fixture::runtime().block_on(async {
        // Retain a real old head throughout execution, including all failure cases.
        let mut h = fixture::Harness::at(root.clone()).await;
        let baseline = h.start(10, 10, 0, fixture::contract()).await;
        h.publish(baseline, 10).await?;
        h.finish(10, "SUCCEEDED").await;
        let store = ArtifactStore::open(&root)?;
        let inputs = request["inputs"].as_array().ok_or("Expected input array")?.iter()
            .map(|b| { let d=ContentDigest::from_hex(DigestKind::Artifact,b["artifact_digest"].as_str().ok_or("Expected digest")?)?;
                Ok(store.verify(d)?) }).collect::<Result<Vec<_>,Box<dyn Error>>>()?;
        let log_directory = PathBuf::from(request["result_directory"].as_str().ok_or("Expected result directory")?);
        let threads = request["threads"].as_str().ok_or("Expected threads")?.parse()?;
        let launch = Launch { python, operation:Operation::Execute, request_schema:"PolarsExecutionRequestV1", request,
            policy:Policy { operation_timeout:Some(Duration::from_secs(60)), termination_grace:Duration::from_millis(100), ..Policy::default() },
            log_directory:Some(log_directory), redact:vec![], timing:None, reservation:None, threads };
        let completion = tf_exec::polars::run(&store, launch, &inputs, Cancellation::default(), tf_exec::discovery::random_id()?.parse()?);
        let head = h.scalar("SELECT generation FROM dataset_heads").await;
        let versions = h.scalar("SELECT count(*) FROM dataset_versions").await;
        let events = h.scalar("SELECT count(*) FROM events").await;
        if head != 1 || versions != 1 || events != 1 { return Err("Execution changed the old publication".into()); }
        let evidence = completion.report.as_ref().map(|r|json!({"outcome":format!("{:?}",r.outcome),
            "stdout":String::from_utf8_lossy(&r.stdout.bytes),"stderr":String::from_utf8_lossy(&r.stderr.bytes),
            "threads":r.threads, "terminal":r.terminal.as_ref().map(|f|f.as_json()),"logs_complete":r.logs_complete}));
        let value=match completion.candidate {Ok(c)=>json!({"ok":true,"manifest":c.manifest(),"digest":c.digest()?.hex()}),
            Err(e)=>json!({"ok":false,"error":e.to_string()})};
        println!("{}",json!({"execution":value,"worker":evidence,"head_generation":head,"versions":versions,"events":events}));
        // Fixture Drop removes the workspace; preserve output for independent inspection.
        h.handoff().await;
        Ok::<(),Box<dyn Error>>(())
    })
}
