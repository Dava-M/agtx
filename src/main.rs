use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;

mod config;

/// Read-only terminal observer for agent-driven changes.
#[derive(Parser)]
#[command(name = "agtx", version, about)]
struct Args {
    /// Repository root to observe.
    #[arg(default_value = ".")]
    path: PathBuf,

    /// TOML config file to use instead of the default location.
    #[arg(long)]
    config: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let _config = config::load(args.config.as_deref())?;
    let label = config_label(args.config.as_deref())?;
    println!("agtx config ok: {label}");
    Ok(())
}

/// The config path to report: the explicit `--config` path, the default path
/// when a file exists there, or "defaults" when it does not.
fn config_label(explicit: Option<&Path>) -> Result<String> {
    match explicit {
        Some(path) => Ok(path.display().to_string()),
        None => {
            let path = config::default_path()?;
            if path.is_file() {
                Ok(path.display().to_string())
            } else {
                Ok("defaults".to_string())
            }
        }
    }
}
