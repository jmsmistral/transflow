//! Workspace creation is composition: configuration stays in tf-catalog; ownership in tf-exec.
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
};
use tf_catalog::{
    RegistrySnapshot,
    workspace::{LocalConfig, WorkspaceConfig, read_authoring},
};
use tf_domain::WorkspaceId;
use tf_exec::ownership::{CoordinatorMode, RuntimeOwner};

#[derive(Debug, thiserror::Error)]
pub(crate) enum InitError {
    #[error("Initialization could not access a workspace file; check permissions and retry")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Config(#[from] tf_catalog::workspace::ConfigError),
    #[error(transparent)]
    Owner(#[from] tf_exec::ownership::OwnershipError),
    #[error(
        "The workspace is partially initialized or has conflicting files; preserve its configuration and registry, restore the missing counterpart from backup, and retry"
    )]
    Partial,
    #[error(
        "An initialization path is a symlink or special file; use ordinary workspace directories and files"
    )]
    UnsafePath,
    #[error(
        "A nested .transflow/.gitignore can hide the registry; remove that conflict before initialization"
    )]
    Ignore,
    #[error("The durable registry is invalid; correct catalog.toml before retrying")]
    Registry,
}
const IGNORE: &str = "# Transflow: keep durable identity; ignore machine-local state\n!/.transflow/\n!/.transflow/catalog.toml\n/.transflow/runtime/\n/transflow.local.toml\n";
fn regular(path: &Path) -> Result<bool, InitError> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_file() && !m.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(InitError::UnsafePath),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn directory(path: &Path) -> Result<(), InitError> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(InitError::UnsafePath),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o700).create(path)?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}
fn create(path: &Path, bytes: &[u8]) -> Result<(), InitError> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}
fn uuid() -> Result<WorkspaceId, InitError> {
    let mut bytes = [0; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    Ok(WorkspaceId::from_bytes(bytes))
}
/// Generate only the initial immutable authoring identity. Existing configurations are never rewritten.
pub(crate) fn initialize(destination: &Path) -> Result<String, InitError> {
    // Parents must already exist: no recursive traversal/creation of unrelated directories.
    directory(destination)?;
    let root = destination.canonicalize()?;
    let config_path = root.join("workspace.toml");
    let registry = root.join(".transflow/catalog.toml");
    let has_config = regular(&config_path)?;
    // Validate every existing parent before touching anything beneath it.
    let authoring = root.join(".transflow");
    if let Ok(m) = fs::symlink_metadata(&authoring)
        && (!m.is_dir() || m.file_type().is_symlink())
    {
        return Err(InitError::UnsafePath);
    }
    let has_registry = regular(&registry)?;
    if !has_config
        && !has_registry
        && authoring.is_dir()
        && fs::read_dir(&authoring)?.next().is_some()
    {
        return Err(InitError::Partial);
    }
    if has_config != has_registry {
        return Err(InitError::Partial);
    }
    let (config, text) = if has_config {
        let text = read_authoring(&config_path)?;
        let config = WorkspaceConfig::parse(&text)?;
        RegistrySnapshot::parse(config.id(), &read_authoring(&registry)?)
            .map_err(|_| InitError::Registry)?;
        (config, text)
    } else {
        let text = format!(
            "format_version = 1\nworkspace_id = \"{}\"\nsource_roots = [\"src\"]\ndefault_data_branch = \"master\"\n\n[python]\nversion = \"3.14\"\nrequirements = \"requirements.in\"\nlock = \"requirements.lock\"\n",
            uuid()?
        );
        (WorkspaceConfig::parse(&text)?, text)
    };
    if regular(&root.join("transflow.local.toml"))? {
        LocalConfig::parse(&read_authoring(&root.join("transflow.local.toml"))?)?;
    }
    if regular(&authoring.join(".gitignore"))? {
        return Err(InitError::Ignore);
    }
    let ignore = root.join(".gitignore");
    let ignore_text = if regular(&ignore)? {
        read_authoring(&ignore)?
    } else {
        String::new()
    };
    let requirements = root.join(config.dependency_paths().0);
    let has_requirements = regular(&requirements)?;
    if has_config {
        if !has_requirements {
            return Err(InitError::Partial);
        }
        tf_catalog::workspace::Workspace::load(&root, Some(&root))?;
    }

    // Existing config is authoritative. Never silently fill partial custom source layouts.
    if !has_config
        && let Ok(m) = fs::symlink_metadata(root.join("src"))
        && (!m.is_dir() || m.file_type().is_symlink())
    {
        return Err(InitError::UnsafePath);
    }
    directory(&authoring)?;
    directory(&authoring.join("runtime"))?;
    let _owner = RuntimeOwner::acquire(&root, config.id(), CoordinatorMode::Temporary)?;
    // Revalidate after taking ownership, so two initializers cannot regenerate an identity.
    if regular(&config_path)? != has_config || regular(&registry)? != has_registry {
        return Err(InitError::Partial);
    }
    if has_config && (read_authoring(&config_path)? != text) {
        return Err(InitError::Partial);
    }
    if !has_config {
        directory(&root.join("src"))?;
        if !has_requirements {
            create(
                &requirements,
                b"# Qualified initial transform engine\npolars==1.44.2\n",
            )?;
        }
        create(&registry, b"format_version = 1\n")?;
        create(&config_path, text.as_bytes())?;
    }
    // Preserve all user rules. Final exact rules override earlier broad parent ignores.
    if !ignore_text.ends_with(IGNORE) {
        if regular(&ignore)? {
            if read_authoring(&ignore)? != ignore_text {
                return Err(InitError::Partial);
            }
            let mut f = OpenOptions::new().append(true).open(&ignore)?;
            if !ignore_text.is_empty() && !ignore_text.ends_with('\n') {
                f.write_all(b"\n")?;
            }
            f.write_all(IGNORE.as_bytes())?;
            f.sync_all()?;
        } else {
            create(&ignore, IGNORE.as_bytes())?;
        }
    }
    File::open(&root)?.sync_all()?;
    Ok(format!(
        "Workspace {}: {}",
        config.id(),
        if has_config {
            "already initialized; identity and configuration preserved"
        } else {
            "initialized; write transforms under src/ after preparing the environment"
        }
    ))
}
