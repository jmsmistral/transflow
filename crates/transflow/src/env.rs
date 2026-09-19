//! Compose explicit workspace, ownership and environment services for CLI operations.
use std::path::{Path, PathBuf};
use tf_catalog::workspace::Workspace;
use tf_exec::{
    environment::{EnvironmentAction, EnvironmentRequest},
    ownership::{CoordinatorMode, RuntimeOwner},
};
#[derive(Debug, thiserror::Error)]
pub(crate) enum EnvError {
    #[error(transparent)]
    Config(#[from] tf_catalog::workspace::ConfigError),
    #[error(transparent)]
    Owner(#[from] tf_exec::ownership::OwnershipError),
    #[error(transparent)]
    Service(#[from] tf_exec::environment::EnvironmentError),
    #[error("Cannot locate the current working directory")]
    Io(#[from] std::io::Error),
    #[error("Choose env lock, env sync or env check; use --python for prepared tooling")]
    Usage,
}
pub(crate) fn execute(
    matches: &clap::ArgMatches,
    explicit: Option<&String>,
) -> Result<String, EnvError> {
    let current = std::env::current_dir()?;
    let workspace = Workspace::load(&current, explicit.map(Path::new))?;
    let config = workspace.config();
    let action = match matches.subcommand_name() {
        Some("lock") => EnvironmentAction::Lock,
        Some("sync") => EnvironmentAction::Sync,
        Some("check") => EnvironmentAction::Check,
        _ => return Err(EnvError::Usage),
    };
    let python = matches
        .get_one::<String>("python")
        .map(PathBuf::from)
        .or_else(|| workspace.local().interpreter().map(Path::to_owned))
        .unwrap_or_else(|| PathBuf::from(format!("python{}", config.python_version())));
    let python = if python.is_relative() && python.components().count() > 1 {
        current.join(python)
    } else {
        python
    };
    let request = EnvironmentRequest {
        action,
        requirements: config.dependency_paths().0.into(),
        lock: config.dependency_paths().1.into(),
        minor: config.python_version().into(),
        runtime_wheel: matches
            .get_one::<String>("runtime-wheel")
            .map(|p| current.join(p)),
        runtime_version: env!("CARGO_PKG_VERSION").replace("-dev", ".dev0"),
        wheelhouse: matches
            .get_one::<String>("wheelhouse")
            .map(|p| current.join(p)),
        offline: matches.get_flag("no-index"),
    };
    let mut owner =
        RuntimeOwner::acquire(workspace.root(), config.id(), CoordinatorMode::Temporary)?;
    Ok(tf_exec::environment::run(&mut owner, &python, &request)?)
}
