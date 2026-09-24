//! Bounded loopback CLI transport with private session authentication. No browser API.
use crate::{
    build_cli,
    build_plan::{Error, failure},
    dispatch::CancelCommand,
};
use clap::ArgMatches;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Duration,
};
use tf_domain::{BuildId, WorkspaceId};
use tf_exec::ownership::{self, RuntimeOwner};
pub(crate) type Mailbox = Arc<Mutex<Receiver<CancelCommand>>>;
pub(crate) struct Submission {
    pub args: Vec<String>,
    pub reply: SyncSender<Value>,
}
pub(crate) struct Endpoint {
    pub commands: Mailbox,
    pub providers: crate::provider::Mailbox,
    pub requests: Receiver<Submission>,
    pub busy: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
fn read(stream: &mut TcpStream) -> Result<Value, Error> {
    let mut size = [0; 4];
    stream.read_exact(&mut size).map_err(failure)?;
    let size = u32::from_be_bytes(size) as usize;
    if size > 1024 * 1024 {
        return Err(failure("coordinator message exceeds its limit"));
    }
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes).map_err(failure)?;
    serde_json::from_slice(&bytes).map_err(failure)
}
fn write(stream: &mut TcpStream, value: &Value) -> Result<(), Error> {
    let bytes = serde_json::to_vec(value).map_err(failure)?;
    if bytes.len() > 1024 * 1024 {
        return Err(failure("coordinator response exceeds its limit"));
    }
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .map_err(failure)?;
    stream.write_all(&bytes).map_err(failure)
}
pub(crate) fn endpoint(owner: &mut RuntimeOwner, persistent: bool) -> Result<Endpoint, Error> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(failure)?;
    listener.set_nonblocking(true).map_err(failure)?;
    owner.register_endpoint(&listener).map_err(failure)?;
    let token = owner.registration().nonce().to_owned();
    let session = owner.registration().session().map_err(failure)?.to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let busy = Arc::new(AtomicBool::new(!persistent));
    let occupied = busy.clone();
    let (commands, rx) = mpsc::sync_channel(16);
    let (providers, provider_rx) = mpsc::sync_channel(16);
    let (requests, submissions) = mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || {
        while !stopped.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, peer)) if peer.ip().is_loopback() => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                    let result = (|| -> Result<Value, Error> {
                        let request = read(&mut stream)?;
                        if request["version"] != 1
                            || request["session"] != session
                            || request["token"] != token
                        {
                            return Err(failure("coordinator authentication refused"));
                        }
                        if request.as_object().is_none_or(|o| {
                            o.keys().any(|k| {
                                !matches!(
                                    k.as_str(),
                                    "version"
                                        | "session"
                                        | "token"
                                        | "operation"
                                        | "build"
                                        | "args"
                                        | "provider"
                                )
                            })
                        }) {
                            return Err(failure("unsupported coordinator request fields"));
                        }
                        match request["operation"].as_str() {
                            Some("provider") => {
                                let request = serde_json::from_value(request["provider"].clone())
                                    .map_err(failure)?;
                                let (tx, rx) = mpsc::sync_channel(1);
                                providers
                                    .try_send(crate::provider::Command { request, reply: tx })
                                    .map_err(|_| failure("provider metadata queue is full"))?;
                                let result=rx.recv_timeout(Duration::from_secs(10)).map_err(|_|failure("Provider metadata request timed out; retry when the coordinator is ready"))?.map_err(failure)?;
                                Ok(json!({"ok":true,"result":result}))
                            }
                            Some("cancel") => {
                                let build = request["build"]
                                    .as_str()
                                    .ok_or_else(|| failure("missing build"))?
                                    .parse()
                                    .map_err(failure)?;
                                let (tx, rx) = mpsc::sync_channel(1);
                                commands
                                    .try_send(CancelCommand { build, reply: tx })
                                    .map_err(|_| failure("coordinator control queue is full"))?;
                                let result=rx.recv_timeout(Duration::from_secs(10)).map_err(|_|failure("cancellation acknowledgement timed out; inspect build state"))?.map_err(failure)?;
                                Ok(json!({"ok":true,"disposition":format!("{result:?}")}))
                            }
                            Some("submit") if persistent => {
                                let args: Vec<String> =
                                    serde_json::from_value(request["args"].clone())
                                        .map_err(failure)?;
                                if occupied.swap(true, Ordering::AcqRel) {
                                    return Err(failure(
                                        "coordinator is executing another build; retry when it finishes",
                                    ));
                                }
                                if args.len() > 10000 {
                                    occupied.store(false, Ordering::Release);
                                    return Err(failure("too many build arguments"));
                                }
                                let (tx, rx) = mpsc::sync_channel(1);
                                if requests.try_send(Submission { args, reply: tx }).is_err() {
                                    occupied.store(false, Ordering::Release);
                                    return Err(failure("coordinator submission queue is full"));
                                }
                                // Preparation uses the normal bounded discovery deadline. Disconnect never cancels a persistent submission.
                                loop {
                                    match rx.recv_timeout(Duration::from_millis(100)) {
                                        Ok(v) => return Ok(v),
                                        Err(mpsc::RecvTimeoutError::Timeout)
                                            if !stopped.load(Ordering::Acquire) => {}
                                        _ => {
                                            return Err(failure(
                                                "coordinator stopped before acceptance",
                                            ));
                                        }
                                    }
                                }
                            }
                            _ => Err(failure("unsupported coordinator operation")),
                        }
                    })();
                    let response =
                        result.unwrap_or_else(|e| json!({"ok":false,"error":e.to_string()}));
                    let _ = write(&mut stream, &response);
                }
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(_) => break,
            }
        }
    });
    Ok(Endpoint {
        commands: Arc::new(Mutex::new(rx)),
        providers: Arc::new(Mutex::new(provider_rx)),
        requests: submissions,
        busy,
        stop,
        thread: Some(thread),
    })
}
pub(crate) fn call(
    root: &Path,
    workspace: WorkspaceId,
    mut request: Value,
    persistent: bool,
) -> Result<Value, Error> {
    let registration=ownership::discover(root,workspace).map_err(failure)?.ok_or_else(||failure("No persistent coordinator is running. Start transflow serve, or use the default waiting build"))?;
    if persistent {
        registration.mode().require_persistent().map_err(failure)?;
    }
    request["version"] = json!(1);
    request["token"] = json!(registration.nonce());
    request["session"] = json!(registration.session().map_err(failure)?.to_string());
    let mut stream = TcpStream::connect_timeout(&registration.endpoint(), Duration::from_secs(2))
        .map_err(failure)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(failure)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(
            if request["operation"] == "provider" {
                15
            } else {
                3600
            },
        )))
        .map_err(failure)?;
    write(&mut stream, &request)?;
    let response = read(&mut stream)?;
    if response["ok"] != true {
        return Err(failure(
            response["error"]
                .as_str()
                .unwrap_or("coordinator refused request"),
        ));
    }
    Ok(response)
}
pub(crate) fn cancel(
    root: &Path,
    workspace: WorkspaceId,
    build: BuildId,
) -> Result<build_cli::Report, Error> {
    if ownership::discover(root, workspace)
        .map_err(failure)?
        .is_some()
    {
        return Ok(build_cli::report(
            "cancel",
            call(
                root,
                workspace,
                json!({"operation":"cancel","build":build.to_string()}),
                false,
            )?,
            false,
        ));
    }
    let mut owner = RuntimeOwner::acquire(root, workspace, ownership::CoordinatorMode::Temporary)
        .map_err(failure)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(failure)?;
    rt.block_on(crate::recovery::recover(&mut owner))?;
    let disposition = cancel_owned(&mut owner, &rt, build)?;
    Ok(build_cli::report(
        "cancel",
        json!({"id":build.to_string(),"disposition":format!("{disposition:?}")}),
        false,
    ))
}
pub(crate) fn submit(
    root: &Path,
    workspace: WorkspaceId,
    args: &ArgMatches,
    wait: bool,
    json_mode: bool,
) -> Result<build_cli::Report, Error> {
    let mut raw = vec![];
    for key in [
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
        "plan",
    ] {
        if args.value_source(key) != Some(clap::parser::ValueSource::CommandLine) {
            continue;
        }
        if matches!(key, "force" | "no-fallback") {
            raw.push(format!("--{key}"));
            continue;
        }
        for value in args.get_raw(key).into_iter().flatten() {
            if key != "targets" {
                raw.push(format!("--{key}"));
            }
            raw.push(value.to_string_lossy().into_owned());
        }
    }
    let accepted = call(
        root,
        workspace,
        json!({"operation":"submit","args":raw}),
        true,
    )?;
    let build: BuildId = accepted["build"]
        .as_str()
        .ok_or_else(|| failure("coordinator omitted build identity"))?
        .parse()
        .map_err(failure)?;
    if !wait {
        return Ok(build_cli::report(
            "run",
            json!({"id":build.to_string(),"state":"ACCEPTED"}),
            false,
        ));
    }
    wait_for(root, workspace, build, json_mode)
}
fn wait_for(
    root: &Path,
    workspace: WorkspaceId,
    build: BuildId,
    json_mode: bool,
) -> Result<build_cli::Report, Error> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(failure)?;
    let canceled = Arc::new(AtomicBool::new(false));
    let signal = canceled.clone();
    rt.block_on(async {
        let listener = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                signal.store(true, Ordering::Release);
            }
        });
        let mut sent = false;
        let mut states = std::collections::BTreeMap::new();
        loop {
            if canceled.load(Ordering::Acquire) && !sent {
                cancel(root, workspace, build)?;
                sent = true;
            }
            let value = build_cli::snapshot(root, workspace, build).await?;
            if !json_mode {
                for j in value["jobs"]
                    .as_array()
                    .ok_or_else(|| failure("missing jobs"))?
                {
                    let id = j["id"].to_string();
                    let state = j["state"].to_string();
                    if states.insert(id.clone(), state.clone()).as_ref() != Some(&state) {
                        eprintln!("{id} {state}");
                    }
                }
            }
            if !matches!(value["state"].as_str(), Some("QUEUED" | "RUNNING")) {
                listener.abort();
                return Ok(build_cli::report("run", value, true));
            }
            if ownership::discover(root, workspace)
                .map_err(failure)?
                .is_none()
            {
                return Err(failure(
                    "coordinator stopped; restart transflow serve to reconcile retained execution",
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
}

pub(crate) fn replay(
    root: &Path,
    workspace: WorkspaceId,
    original: BuildId,
    destination: &tf_domain::BranchName,
    json_mode: bool,
) -> Result<build_cli::Report, Error> {
    let accepted = call(
        root,
        workspace,
        json!({"operation":"submit","args":["replay",original.to_string(),"--branch",destination.to_string()]}),
        true,
    )?;
    let build = accepted["build"]
        .as_str()
        .ok_or_else(|| failure("coordinator omitted replay identity"))?
        .parse()
        .map_err(failure)?;
    let mut report = wait_for(root, workspace, build, json_mode)?;
    report.value["operation"] = json!("replay");
    Ok(report)
}

/// Persist cancellation and settle an untouched accepted queue without starting producers.
pub(crate) fn cancel_owned(
    owner: &mut RuntimeOwner,
    rt: &tokio::runtime::Runtime,
    build: BuildId,
) -> Result<tf_domain::execution::CancelRequest, Error> {
    let workspace = owner.workspace_id().map_err(failure)?;
    let session = owner.registration().session().map_err(failure)?;
    let (disposition, queued) = rt.block_on(async {
        let mut owned = owner.open_store().await.map_err(failure)?;
        let store = owned.repository().map_err(failure)?;
        let queued = store.build_is_queued(build).await.map_err(failure)?;
        let disposition = store
            .cancel_publications(workspace, session, build)
            .await
            .map_err(failure)?;
        owned.close().await.map_err(failure)?;
        Ok::<_, Error>((disposition, queued))
    })?;
    if queued {
        crate::dispatch::cancel_unstarted(owner, rt, build).map_err(failure)?;
    }
    Ok(disposition)
}
