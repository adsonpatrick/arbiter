mod doctor;
mod init;
mod start;
mod status;
mod uninstall;

use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use arbiter_adapter_codex::InstallReceipt;
use arbiter_core::{config::BaselineTarget, health::DaemonIdentity};
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Parser)]
#[command(name = "arbiter", version, about = "Local compute governor for Codex")]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init {
        #[arg(value_enum)]
        target: InitTarget,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        port: Option<u16>,
    },
    Start {
        #[arg(long, hide = true)]
        foreground: bool,
    },
    Status,
    Doctor,
    Uninstall {
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum InitTarget {
    Codex,
}

#[derive(Debug, Clone)]
pub(crate) struct Paths {
    pub arbiter_home: PathBuf,
    pub codex_config: PathBuf,
    pub config: PathBuf,
    pub database: PathBuf,
    pub receipt: PathBuf,
    pub server: PathBuf,
    pub stop: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LocalConfig {
    pub schema_version: u16,
    pub mode: String,
    pub baseline: BaselineTarget,
    pub port: u16,
    pub codex_config: PathBuf,
    pub bind_address: String,
    pub remote_export: bool,
    pub instance_id: String,
}

pub(crate) type ServerMetadata = DaemonIdentity;

pub(crate) async fn run(cli: Cli) -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    match cli.command {
        Command::Init { target, yes, port } => init::run(&paths, target, yes, port).await,
        Command::Start { foreground } => start::run(&paths, foreground).await,
        Command::Status => status::run(&paths).await,
        Command::Doctor => doctor::run(&paths).await,
        Command::Uninstall { yes } => uninstall::run(&paths, yes).await,
    }
}

impl Paths {
    fn resolve() -> anyhow::Result<Self> {
        let user_home = env::var_os("USERPROFILE")
            .or_else(|| env::var_os("HOME"))
            .map(PathBuf::from)
            .context("USERPROFILE or HOME is required")?;
        let arbiter_home =
            env::var_os("ARBITER_HOME").map_or_else(|| user_home.join(".arbiter"), PathBuf::from);
        let codex_home =
            env::var_os("CODEX_HOME").map_or_else(|| user_home.join(".codex"), PathBuf::from);
        Ok(Self {
            config: arbiter_home.join("config.json"),
            database: arbiter_home.join("arbiter.db"),
            receipt: arbiter_home.join("install-receipt.json"),
            server: arbiter_home.join("server.json"),
            stop: arbiter_home.join("stop"),
            codex_config: codex_home.join("config.toml"),
            arbiter_home,
        })
    }
}

pub(crate) fn default_config(paths: &Paths, port: u16, instance_id: String) -> LocalConfig {
    LocalConfig {
        schema_version: 1,
        mode: "passthrough".to_owned(),
        baseline: BaselineTarget::m0(),
        port,
        codex_config: paths.codex_config.clone(),
        bind_address: "127.0.0.1".to_owned(),
        remote_export: false,
        instance_id,
    }
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> anyhow::Result<T> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

pub(crate) fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let parent = path.parent().context("metadata path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let bytes = serde_json::to_vec_pretty(value).context("serialize local metadata")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&bytes)?;
    temporary.write_all(b"\n")?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub(crate) fn read_config(paths: &Paths) -> anyhow::Result<LocalConfig> {
    let config: LocalConfig = read_json(&paths.config)?;
    if config.schema_version != 1
        || config.mode != "passthrough"
        || config.baseline != BaselineTarget::m0()
        || config.codex_config != paths.codex_config
        || config.bind_address != "127.0.0.1"
        || config.remote_export
        || config.port == 0
        || config.instance_id.is_empty()
    {
        bail!("local configuration violates the M0 contract");
    }
    Ok(config)
}

pub(crate) fn read_receipt(paths: &Paths) -> anyhow::Result<InstallReceipt> {
    read_json(&paths.receipt)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command, InitTarget};

    #[test]
    fn m0_cli_exposes_only_codex_as_an_init_target() {
        let cli = Cli::try_parse_from(["arbiter", "init", "codex", "--yes"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Init {
                target: InitTarget::Codex,
                yes: true,
                port,
            } if port.is_none()
        ));
        assert!(Cli::try_parse_from(["arbiter", "init", "openai", "--yes"]).is_err());
    }
}
