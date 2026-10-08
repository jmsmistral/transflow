//! Scheduling CLI adapters over the same guarded services as the loopback API.
use crate::build_plan::{Error, failure};
use clap::{Arg, ArgAction, ArgMatches, Command};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap, fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, path::Path,
};
use tf_domain::{ScheduleId, ScheduleOccurrenceId};
use tf_exec::ownership::{CoordinatorMode, RuntimeOwner};

pub(crate) fn command() -> Command {
    let mut root = Command::new("schedule")
        .about("Create, inspect and control durable schedules")
        .disable_help_flag(true)
        .subcommand_required(true);
    for name in [
        "create", "update", "list", "show", "run", "pause", "resume", "history", "metrics",
        "export", "delete",
    ] {
        let mut command = Command::new(name).disable_help_flag(true);
        if name != "create" && name != "list" {
            command = command.arg(
                Arg::new("id")
                    .required(true)
                    .value_parser(clap::value_parser!(ScheduleId)),
            );
        }
        if matches!(name, "create" | "update") {
            command = command.arg(Arg::new("file").long("file").required(true));
        }
        if name == "create" {
            command = command.arg(Arg::new("paused").long("paused").action(ArgAction::SetTrue));
        }
        if name == "update" {
            command = command.arg(Arg::new("if-match").long("if-match").required(true));
        } else if matches!(name, "run" | "pause" | "resume" | "delete") {
            command = command.arg(Arg::new("if-match").long("if-match"));
        }
        if matches!(name, "update" | "resume") {
            command = command
                .arg(
                    Arg::new("replay-after-event")
                        .long("replay-after-event")
                        .requires("replay-limit"),
                )
                .arg(
                    Arg::new("replay-limit")
                        .long("replay-limit")
                        .requires("replay-after-event")
                        .value_parser(clap::value_parser!(u32).range(1..=100)),
                );
        }
        if name == "list" {
            command = command.arg(
                Arg::new("after")
                    .long("after")
                    .value_parser(clap::value_parser!(ScheduleId)),
            );
        }
        if name == "history" {
            command = command
                .arg(
                    Arg::new("after")
                        .long("after")
                        .value_parser(clap::value_parser!(ScheduleOccurrenceId)),
                )
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .default_value("50")
                        .value_parser(clap::value_parser!(u16).range(1..=100)),
                );
        }
        if name == "metrics" {
            command = command
                .arg(
                    Arg::new("from-us")
                        .long("from-us")
                        .required(true)
                        .value_parser(clap::value_parser!(i64).range(0..)),
                )
                .arg(
                    Arg::new("to-us")
                        .long("to-us")
                        .required(true)
                        .value_parser(clap::value_parser!(i64).range(1..)),
                )
                .arg(
                    Arg::new("window")
                        .long("window")
                        .default_value("10")
                        .value_parser(clap::value_parser!(u16).range(1..=1000)),
                );
        }
        if name == "export" {
            command = command.arg(Arg::new("path").long("path").required(true));
        }
        if name == "delete" {
            command = command.arg(Arg::new("yes").long("yes").action(ArgAction::SetTrue));
        }
        root = root.subcommand(command);
    }
    root
}
/// Authenticated IPC carries normalized data, never filesystem arguments or credentials.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Intent {
    operation: String,
    id: String,
    definition: Value,
    etag: Option<String>,
    paused: bool,
    key: String,
    replay_after: Option<String>,
    replay_limit: Option<u32>,
}
pub(crate) fn mutate(owner: &mut RuntimeOwner, value: Value) -> Result<Value, Error> {
    let intent: Intent = serde_json::from_value(value).map_err(failure)?;
    intent.id.parse::<ScheduleId>().map_err(failure)?;
    if intent.operation == "run" {
        owner
            .registration()
            .mode()
            .require_persistent()
            .map_err(failure)?;
    }
    let (method, path, body) = match intent.operation.as_str() {
        "create" => (
            "POST",
            "/api/v1/schedules".into(),
            json!({"id":intent.id,"paused":intent.paused,"definition":intent.definition}),
        ),
        "update" => (
            "PUT",
            format!("/api/v1/schedules/{}", intent.id),
            intent.definition,
        ),
        "run" | "pause" | "resume" | "delete" => (
            "POST",
            format!("/api/v1/schedules/{}/{}", intent.id, intent.operation),
            if intent.operation == "resume" && intent.replay_after.is_some() {
                json!({"replay_after_event":intent.replay_after,"replay_limit":intent.replay_limit})
            } else {
                json!({})
            },
        ),
        _ => return Err(failure("Unsupported schedule operation")),
    };
    let mut query = BTreeMap::new();
    if intent.operation == "update"
        && let Some(after) = intent.replay_after
    {
        query.insert("replay_after_event".into(), after);
        query.insert(
            "replay_limit".into(),
            intent
                .replay_limit
                .ok_or_else(|| failure("Replay needs an explicit event budget"))?
                .to_string(),
        );
    }
    let key = intent.key.parse().map_err(failure)?;
    let request = tf_api::Request {
        id: key,
        method: method.into(),
        path,
        query,
        body,
        key: Some(key),
        if_match: intent.etag,
    };
    let digest = crate::api_read::digest(&json!([
        request.method,
        request.path,
        request.query,
        request.body,
        request.if_match
    ]))
    .map_err(failure)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(failure)?;
    crate::api_schedules::save(owner, &request, &rt, &digest)
        .map(|reply| reply.data)
        .map_err(failure)
}
fn report(operation: &str, data: Value) -> crate::build_cli::Report {
    let human = serde_json::to_string_pretty(&data).unwrap_or_default();
    crate::build_cli::Report {
        value: json!({"kind":"schedule","operation":operation,"data":data}),
        status: tf_domain::diagnostic::ExitStatus::Success,
        human,
    }
}
pub(crate) fn execute(
    args: &ArgMatches,
    explicit: Option<&String>,
) -> Result<crate::build_cli::Report, Error> {
    let current = std::env::current_dir().map_err(failure)?;
    let workspace = tf_catalog::workspace::Workspace::load(&current, explicit.map(Path::new))
        .map_err(failure)?;
    let root = workspace.root();
    let wid = workspace.config().id();
    let (operation, args) = args
        .subcommand()
        .ok_or_else(|| failure("Select a schedule operation"))?;
    let id = if matches!(operation, "create" | "list") {
        None
    } else {
        args.get_one::<ScheduleId>("id").copied()
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(failure)?;
    if matches!(
        operation,
        "list" | "show" | "history" | "metrics" | "export"
    ) || (operation == "delete" && !args.get_flag("yes"))
    {
        let data = rt.block_on(async {
            let mut reader =
                tf_store::Reader::open_existing(&root.join(".transflow/runtime/catalog.sqlite"))
                    .await
                    .map_err(failure)?;
            let data = match operation {
                "list" => reader
                    .schedules(
                        wid,
                        &args
                            .get_one::<ScheduleId>("after")
                            .map(ToString::to_string)
                            .unwrap_or_default(),
                    )
                    .await
                    .map_err(failure)?,
                "history" => reader
                    .schedule_history(
                        wid,
                        id.ok_or_else(|| failure("Missing schedule"))?,
                        args.get_one::<ScheduleOccurrenceId>("after").copied(),
                        usize::from(
                            *args
                                .get_one::<u16>("limit")
                                .ok_or_else(|| failure("Missing page limit"))?,
                        ),
                    )
                    .await
                    .map_err(failure)?,
                "metrics" => reader
                    .schedule_metrics(
                        wid,
                        id.ok_or_else(|| failure("Missing schedule"))?,
                        *args
                            .get_one::<i64>("from-us")
                            .ok_or_else(|| failure("Missing From time"))?,
                        *args
                            .get_one::<i64>("to-us")
                            .ok_or_else(|| failure("Missing To time"))?,
                        usize::from(
                            *args
                                .get_one::<u16>("window")
                                .ok_or_else(|| failure("Missing mean window"))?,
                        ),
                    )
                    .await
                    .map_err(failure)?,
                _ => reader
                    .schedule(wid, id.ok_or_else(|| failure("Missing schedule"))?)
                    .await
                    .map_err(failure)?
                    .ok_or_else(|| failure("The requested schedule is unavailable"))?,
            };
            reader.close().await.map_err(failure)?;
            Ok::<_, Error>(data)
        })?;
        if operation == "export" {
            let definition =
                tf_protocol::schedule::Definition::decode(&data["definition"]).map_err(failure)?;
            let bytes = serde_json::to_vec_pretty(&definition).map_err(failure)?;
            let path = args
                .get_one::<String>("path")
                .ok_or_else(|| failure("Choose an export path"))?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
                .map_err(|_| {
                    failure(
                        "The export file already exists or cannot be created; choose a new path",
                    )
                })?;
            file.write_all(&bytes)
                .and_then(|_| file.write_all(b"\n"))
                .and_then(|_| file.sync_all())
                .map_err(failure)?;
            return Ok(report(
                operation,
                json!({"id":data["id"],"path":path,"definition":definition}),
            ));
        }
        if operation == "delete" {
            return Ok(report(
                operation,
                json!({"id":data["id"],"deleted":false,"preview":true,"name":data["definition"]["name"],"paused":data["paused"],"confirmation":"Use --yes to tombstone this definition; history and active builds remain"}),
            ));
        }
        return Ok(report(operation, data));
    }
    let id = match id {
        Some(id) => id,
        None => crate::build_plan::id()?,
    };
    let definition = if matches!(operation, "create" | "update") {
        let path = args
            .get_one::<String>("file")
            .ok_or_else(|| failure("Choose a definition file"))?;
        if std::fs::metadata(path).map_err(failure)?.len() > 262144 {
            return Err(failure("Schedule definition files are limited to 256 KiB"));
        }
        let bytes = std::fs::read(path)
            .map_err(|_| failure("The schedule definition file could not be read"))?;
        if bytes.len() > 262144 {
            return Err(failure("The schedule definition exceeds 256 KiB"));
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| failure("The schedule definition must be valid JSON"))?;
        tf_protocol::schedule::Definition::decode(&value).map_err(|_| {
            failure("The schedule definition has invalid fields, identities or conditions")
        })?;
        value
    } else {
        Value::Null
    };
    let etag = if operation == "create" {
        None
    } else if let Some(guard) = args.get_one::<String>("if-match") {
        Some(guard.clone())
    } else {
        Some(rt.block_on(async {
            let mut reader =
                tf_store::Reader::open_existing(&root.join(".transflow/runtime/catalog.sqlite"))
                    .await
                    .map_err(failure)?;
            let value = reader
                .schedule(wid, id)
                .await
                .map_err(failure)?
                .ok_or_else(|| failure("The requested schedule is unavailable"))?;
            reader.close().await.map_err(failure)?;
            value["etag"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| failure("The schedule edit guard is unavailable"))
        })?)
    };
    let intent = Intent {
        operation: operation.into(),
        id: id.to_string(),
        definition,
        etag,
        paused: operation == "create" && args.get_flag("paused"),
        key: crate::build_plan::id::<tf_domain::RequestId>()?.to_string(),
        replay_after: if matches!(operation, "update" | "resume") {
            args.get_one::<String>("replay-after-event").cloned()
        } else {
            None
        },
        replay_limit: if matches!(operation, "update" | "resume") {
            args.get_one::<u32>("replay-limit").copied()
        } else {
            None
        },
    };
    let value = serde_json::to_value(intent).map_err(failure)?;
    let data = if tf_exec::ownership::discover(root, wid)
        .map_err(failure)?
        .is_some()
    {
        crate::build_transport::call(
            root,
            wid,
            json!({"operation":"schedule","schedule":value}),
            true,
        )?["data"]
            .clone()
    } else {
        if operation == "run" {
            return Err(failure(
                "Start transflow serve before running a schedule; no background service is installed",
            ));
        }
        let mut owner =
            RuntimeOwner::acquire(root, wid, CoordinatorMode::Temporary).map_err(failure)?;
        rt.block_on(crate::recovery::recover(&mut owner))?;
        mutate(&mut owner, value)?
    };
    Ok(report(operation, data))
}
