//! Public execution adapters over the shared planning, dispatch and history services.
use crate::build_plan::{self, Error, failure};
use clap::{Arg, ArgAction, ArgMatches, Command};
use serde_json::{Value, json};
use std::path::Path;
use tf_domain::{BranchName, BuildId, RequestId};
use tf_exec::{
    ownership::{CoordinatorMode, RuntimeOwner},
    supervisor::Cancellation,
};

pub(crate) fn command() -> Command {
    let base = crate::inspection_cli::commands().remove(0);
    base.name("build")
        .about("Build datasets, inspect execution and replay retained history")
        .subcommand_negates_reqs(true)
        .args_conflicts_with_subcommands(true)
        .mut_arg("targets", |a| {
            a.required_unless_present_any(["target", "plan", "help"])
        })
        .arg(
            Arg::new("plan")
                .long("plan")
                .value_parser(clap::value_parser!(RequestId))
                .conflicts_with_all([
                    "targets",
                    "target",
                    "python",
                    "branch",
                    "git-ref",
                    "mode",
                    "boundary-policy",
                    "force",
                    "no-fallback",
                    "fallback",
                    "timeout-seconds",
                    "validation-timeout-seconds",
                    "boundary",
                    "exclude",
                    "refresh-source",
                    "pin",
                    "param",
                ]),
        )
        .arg(
            Arg::new("wait")
                .long("wait")
                .action(ArgAction::SetTrue)
                .conflicts_with("no-wait"),
        )
        .arg(
            Arg::new("no-wait")
                .long("no-wait")
                .action(ArgAction::SetTrue),
        )
        .subcommand(
            Command::new("list")
                .disable_help_flag(true)
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .default_value("50")
                        .value_parser(clap::value_parser!(u16).range(1..=100)),
                )
                .arg(
                    Arg::new("cursor")
                        .long("cursor")
                        .value_parser(clap::value_parser!(BuildId)),
                ),
        )
        .subcommands(
            ["show", "cancel", "logs", "replay"]
                .into_iter()
                .map(|name| {
                    let mut c = Command::new(name).disable_help_flag(true).arg(
                        Arg::new("id")
                            .required_unless_present("help")
                            .value_parser(clap::value_parser!(BuildId)),
                    );
                    if name == "logs" {
                        c = c.arg(Arg::new("follow").long("follow").action(ArgAction::SetTrue));
                    }
                    if name == "replay" {
                        c = c.arg(
                            Arg::new("branch")
                                .long("branch")
                                .required_unless_present("help")
                                .value_parser(clap::value_parser!(BranchName)),
                        );
                    }
                    c
                }),
        )
}
pub(crate) struct Report {
    pub value: Value,
    pub status: tf_domain::diagnostic::ExitStatus,
    pub human: String,
}
pub(crate) fn report(operation: &str, data: Value, wait: bool) -> Report {
    use tf_domain::diagnostic::ExitStatus;
    let status = if wait {
        match data["state"].as_str() {
            Some("SUCCEEDED") => ExitStatus::Success,
            Some("CANCELED") => ExitStatus::Interrupted,
            _ => ExitStatus::Failure,
        }
    } else {
        ExitStatus::Success
    };
    let mut human = if operation == "run" || operation == "replay" {
        format!(
            "Build {}: {}",
            data["id"].as_str().unwrap_or("unknown"),
            data["state"].as_str().unwrap_or("ACCEPTED")
        )
    } else {
        serde_json::to_string_pretty(&data).unwrap_or_default()
    };
    if matches!(operation, "run" | "replay") {
        for job in data["jobs"].as_array().into_iter().flatten() {
            let checks = job["attempts"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|a| a["checks"].as_array().into_iter().flatten())
                .chain(
                    job["cached_from"]["checks"]
                        .as_array()
                        .into_iter()
                        .flatten(),
                );
            for check in checks {
                if check["outcome"] == "VIOLATION" && check["policy"]["severity"] == "WARN" {
                    human.push_str(&format!(
                        "\nWarning: check {} on dataset {} recorded a data violation.",
                        check["name"], job["dataset"]
                    ));
                }
            }
        }
    }
    Report {
        value: json!({"kind":"execution","operation":operation,"data":data}),
        status,
        human,
    }
}
fn runtime() -> Result<tokio::runtime::Runtime, Error> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(failure)
}
pub(crate) async fn snapshot(
    root: &Path,
    workspace: tf_domain::WorkspaceId,
    build: BuildId,
) -> Result<Value, Error> {
    let mut reader =
        tf_store::Reader::open_existing(&root.join(".transflow/runtime/catalog.sqlite"))
            .await
            .map_err(failure)?;
    reader.build_report(workspace, build).await.map_err(failure)
}
pub(crate) async fn submit(
    owner: RuntimeOwner,
    args: &ArgMatches,
) -> Result<build_plan::Completion<build_plan::Accepted>, Error> {
    if let Some((name, sub)) = args.subcommand() {
        if name == "replay" {
            return crate::replay::prepare(
                owner,
                *sub.get_one::<BuildId>("id")
                    .ok_or_else(|| failure("missing original build"))?,
                sub.get_one::<BranchName>("branch")
                    .ok_or_else(|| failure("missing destination"))?
                    .clone(),
            )
            .await;
        }
        return Err(failure(
            "Only build execution and replay can be submitted to the coordinator",
        ));
    }
    if let Some(plan) = args.get_one::<RequestId>("plan") {
        return build_plan::accept(owner, *plan).await;
    }
    let workspace = tf_catalog::workspace::Workspace::load(
        owner.workspace_root(),
        Some(owner.workspace_root()),
    )
    .map_err(failure)?;
    let source = args.get_one::<String>("git-ref").cloned();
    let branch = tf_catalog::git::output_branch(
        workspace.config(),
        tf_catalog::git::inspect(&workspace)
            .map_err(failure)?
            .as_ref(),
        args.get_one::<BranchName>("branch"),
        source.is_some(),
    )
    .map_err(failure)?;
    let mut targets = args
        .get_many::<String>("targets")
        .into_iter()
        .flatten()
        .chain(args.get_many::<String>("target").into_iter().flatten())
        .cloned()
        .collect::<Vec<_>>();
    targets.sort();
    targets.dedup();
    let (request, options) = crate::inspection_cli::selection(args, branch, source, targets);
    let c = build_plan::prepare_with_options(owner, request, options).await?;
    match c.result {
        Ok(plan) => build_plan::accept(c.owner, plan.id.parse().map_err(failure)?).await,
        Err(e) => Ok(build_plan::Completion {
            owner: c.owner,
            result: Err(e),
        }),
    }
}
pub(crate) async fn execute_build(
    mut owner: RuntimeOwner,
    build: BuildId,
    json_mode: bool,
) -> Result<crate::dispatch::Completion, Error> {
    let endpoint = crate::build_transport::endpoint(&mut owner, false)?;
    execute_commands(owner, build, json_mode, endpoint.commands.clone()).await
}
pub(crate) async fn execute_commands(
    mut owner: RuntimeOwner,
    build: BuildId,
    json_mode: bool,
    commands: crate::build_transport::Mailbox,
) -> Result<crate::dispatch::Completion, Error> {
    // A public build with an explicit budget conservatively reserves that entire
    // budget per worker. This is admission accounting, never a measured RSS bound.
    let memory_estimates = {
        let mut owned = owner.open_store().await.map_err(failure)?;
        let plan = owned
            .repository()
            .map_err(failure)?
            .dispatch_plan(build)
            .await
            .map_err(failure)?;
        owned.close().await.map_err(failure)?;
        let mut estimates = std::collections::BTreeMap::new();
        for write in plan.writes {
            if let Some(mib) =
                plan.context["resources"][&write.dataset]["memory_budget_mib"].as_str()
            {
                let bytes = mib
                    .parse::<u64>()
                    .map_err(failure)?
                    .checked_mul(1024 * 1024)
                    .ok_or_else(|| failure("memory budget overflow"))?;
                estimates.insert(write.job.parse().map_err(failure)?, bytes);
            }
        }
        estimates
    };
    let cancel = Cancellation::default();
    let signal = cancel.clone();
    let listener = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal.cancel();
        }
    });
    let (tx, rx) = std::sync::mpsc::sync_channel::<Value>(64);
    let progress = std::thread::spawn(move || {
        while let Ok(v) = rx.recv() {
            if !json_mode {
                eprintln!(
                    "{} {}",
                    v["job"].as_str().unwrap_or(""),
                    v["state"].as_str().unwrap_or("")
                );
            }
        }
    });
    let result = crate::dispatch::run_with_options(
        owner,
        build,
        cancel,
        crate::dispatch::Options {
            progress: Some(tx),
            commands: Some(commands),
            memory_estimates,
        },
    )
    .await
    .map_err(failure);
    listener.abort();
    progress
        .join()
        .map_err(|_| failure("progress reporting failed"))?;
    result
}
pub(crate) fn execute(
    args: &ArgMatches,
    explicit: Option<&String>,
    json_mode: bool,
) -> Result<Report, Error> {
    let current = std::env::current_dir().map_err(failure)?;
    let workspace = tf_catalog::workspace::Workspace::load(&current, explicit.map(Path::new))
        .map_err(failure)?;
    let root = workspace.root();
    let wid = workspace.config().id();
    let rt = runtime()?;
    if let Some((name, sub)) = args.subcommand() {
        if name == "list" {
            let value = rt.block_on(async {
                let mut r = tf_store::Reader::open_existing(
                    &root.join(".transflow/runtime/catalog.sqlite"),
                )
                .await
                .map_err(failure)?;
                r.builds(
                    wid,
                    *sub.get_one::<u16>("limit")
                        .ok_or_else(|| failure("missing limit"))?,
                    sub.get_one::<BuildId>("cursor").copied(),
                )
                .await
                .map_err(failure)
            })?;
            return Ok(report(name, value, false));
        }
        let build = *sub
            .get_one::<BuildId>("id")
            .ok_or_else(|| failure("missing build identity"))?;
        if name == "show" {
            return Ok(report(
                name,
                rt.block_on(snapshot(root, wid, build))?,
                false,
            ));
        }
        if name == "logs" {
            return crate::build_logs::execute(root, wid, build, sub.get_flag("follow"), json_mode);
        }
        if name == "cancel" {
            return crate::build_transport::cancel(root, wid, build);
        }
        if name == "replay" {
            return crate::replay::execute(
                root,
                wid,
                build,
                sub.get_one::<BranchName>("branch")
                    .ok_or_else(|| failure("missing destination"))?
                    .clone(),
                json_mode,
            );
        }
    }
    if args.get_flag("no-wait") {
        return crate::build_transport::submit(root, wid, args, false, json_mode);
    }
    if tf_exec::ownership::discover(root, wid)
        .map_err(failure)?
        .is_some()
    {
        return crate::build_transport::submit(root, wid, args, true, json_mode);
    }
    let mut owner =
        RuntimeOwner::acquire(root, wid, CoordinatorMode::Temporary).map_err(failure)?;
    rt.block_on(crate::recovery::recover(&mut owner))?;
    // Resume untouched accepted queues before new planning can collide with their reservations.
    loop {
        let queued = rt.block_on(async {
            let mut s = owner.open_store().await.map_err(failure)?;
            let q = s
                .repository()
                .map_err(failure)?
                .next_queued_build()
                .await
                .map_err(failure)?;
            s.close().await.map_err(failure)?;
            Ok::<_, Error>(q)
        })?;
        let Some(queued) = queued else {
            break;
        };
        let c = rt.block_on(execute_build(owner, queued, json_mode))?;
        owner = c.owner;
        c.result.map_err(failure)?;
    }
    let c = rt.block_on(submit(owner, args))?;
    let accepted = c.result?;
    let c = rt.block_on(execute_build(c.owner, accepted.build, json_mode))?;
    c.result.map_err(failure)?;
    Ok(report(
        "run",
        rt.block_on(snapshot(root, wid, accepted.build))?,
        true,
    ))
}
