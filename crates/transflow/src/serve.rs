//! Foreground headless coordinator for CLI submissions. API/UI/schedules compose later.
use crate::{
    build_cli,
    build_plan::{Error, failure},
};
use serde_json::json;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
pub(crate) fn execute(
    explicit: Option<&String>,
    json_mode: bool,
    args: &clap::ArgMatches,
) -> Result<build_cli::Report, Error> {
    let current = std::env::current_dir().map_err(failure)?;
    let workspace = tf_catalog::workspace::Workspace::load(&current, explicit.map(Path::new))
        .map_err(failure)?;
    let resolved = crate::resources::Resolved::new(
        &workspace
            .config()
            .execution_policy(
                &Default::default(),
                &Default::default(),
                &Default::default(),
            )
            .map_err(failure)?,
    )
    .map_err(failure)?;
    let admission = tf_exec::admission::Admission::new(resolved.capacity).map_err(failure)?;
    let mut owner = tf_exec::ownership::RuntimeOwner::acquire(
        workspace.root(),
        workspace.config().id(),
        tf_exec::ownership::CoordinatorMode::Persistent,
    )
    .map_err(failure)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(failure)?;
    rt.block_on(crate::recovery::recover(&mut owner))?;
    let endpoint = crate::build_transport::endpoint(&mut owner, true)?;
    endpoint.busy.store(true, Ordering::Release);
    let stopped = Arc::new(AtomicBool::new(false));
    let signal = stopped.clone();
    let _listener = rt.spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal.store(true, Ordering::Release);
        }
    });
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
        let Some(build) = queued else {
            break;
        };
        if stopped.load(Ordering::Acquire) {
            break;
        }
        let c = rt.block_on(build_cli::execute_commands(
            owner,
            build,
            json_mode,
            endpoint.commands.clone(),
            endpoint.providers.clone(),
            Some(admission.clone()),
        ))?;
        owner = c.owner;
        c.result.map_err(failure)?;
    }
    endpoint.busy.store(false, Ordering::Release);
    let (service, commands) = crate::api::service(
        workspace.root().to_owned(),
        workspace.config().id(),
        endpoint.busy.clone(),
        endpoint.cancel_sender.clone(),
        admission.clone(),
    );
    let assets = args
        .get_one::<String>("ui-dir")
        .map(|p| tf_api::Assets::load(Path::new(p)))
        .transpose()
        .map_err(failure)?;
    let http = tf_api::Server::bind_with_assets(
        std::net::SocketAddr::from((
            [127, 0, 0, 1],
            *args
                .get_one::<u16>("port")
                .ok_or_else(|| failure("missing HTTP port"))?,
        )),
        owner.registration().nonce().to_owned(),
        service.clone(),
        assets,
    )
    .map_err(failure)?;
    owner
        .register_http_endpoint(http.address())
        .map_err(failure)?;
    if args.get_flag("open") {
        let url = http.launch_url().map_err(failure)?;
        let launcher = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        if !std::process::Command::new(launcher)
            .arg(url)
            .status()
            .map_err(failure)?
            .success()
        {
            return Err(failure("Could not open the coordinator browser"));
        }
    }
    if !json_mode && !stopped.load(Ordering::Acquire) {
        eprintln!(
            "Coordinator ready. Builds continue after clients disconnect. Press Ctrl-C to stop."
        );
        eprintln!("HTTP API: http://{}", http.address());
    }
    while !stopped.load(Ordering::Acquire) {
        service.queries.tick();
        crate::provider::drain(&mut owner, &rt, &endpoint.providers)?;
        crate::api::drain(&mut owner, &commands, &endpoint.busy).map_err(failure)?;
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
        if let Some(build) = queued {
            endpoint.busy.store(true, Ordering::Release);
            let c = rt.block_on(build_cli::execute_commands(
                owner,
                build,
                json_mode,
                endpoint.commands.clone(),
                endpoint.providers.clone(),
                Some(admission.clone()),
            ))?;
            owner = c.owner;
            c.result.map_err(failure)?;
            endpoint.busy.store(false, Ordering::Release);
        }
        if let Ok(request) = endpoint.requests.try_recv() {
            let parsed = crate::command().try_get_matches_from(
                std::iter::once("transflow".to_owned())
                    .chain(std::iter::once("build".to_owned()))
                    .chain(request.args),
            );
            let c = match parsed {
                Ok(args) => rt.block_on(build_cli::submit(
                    owner,
                    args.subcommand_matches("build")
                        .ok_or_else(|| failure("missing build request"))?,
                ))?,
                Err(_) => {
                    let _ = request
                        .reply
                        .try_send(json!({"ok":false,"error":"Invalid build arguments"}));
                    endpoint.busy.store(false, Ordering::Release);
                    continue;
                }
            };
            owner = c.owner;
            match c.result {
                Ok(accepted) => {
                    let _ = request
                        .reply
                        .try_send(json!({"ok":true,"build":accepted.build.to_string()}));
                    let c = rt.block_on(build_cli::execute_commands(
                        owner,
                        accepted.build,
                        json_mode,
                        endpoint.commands.clone(),
                        endpoint.providers.clone(),
                        Some(admission.clone()),
                    ))?;
                    owner = c.owner;
                    c.result.map_err(failure)?;
                }
                Err(error) => {
                    let _ = request
                        .reply
                        .try_send(json!({"ok":false,"error":error.to_string()}));
                }
            }
            endpoint.busy.store(false, Ordering::Release);
        }
        while let Ok(command) = endpoint.commands.lock().map_err(failure)?.try_recv() {
            let result = crate::api::cancel_owned(&mut owner, &rt, &command);
            let _ = command.reply.try_send(result.map_err(|e| e.to_string()));
        }
        rt.block_on(async {
            tokio::time::sleep(Duration::from_millis(20)).await;
        });
    }
    drop(http);
    service.queries.cancel_all();
    while service.queries.active() {
        crate::provider::drain(&mut owner, &rt, &endpoint.providers)?;
        service.queries.tick();
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(service);
    drop(endpoint);
    Ok(build_cli::report(
        "serve",
        json!({"state":"STOPPED"}),
        false,
    ))
}
