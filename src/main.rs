use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

mod config;
mod git;
// Consumed by the TUI (Task 3); kept warning-free until then.
#[allow(dead_code)]
mod tree;

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

    let root = git::repo_root(&args.path)?;
    let state = git::status::read(&root)?;
    let visible = git::files::visible(&root, &state)?;
    println!("root: {}", root.display());
    println!("branch: {}", state.branch.as_deref().unwrap_or("(no branch)"));
    println!("files: {}", state.files.len());
    println!("visible: {}", visible.len());
    println!("+{}/-{}", state.additions, state.deletions);
    Ok(())
}

/// The config path to report: the explicit `--config` path, the default path
/// when a file exists there, or "defaults" when it does not.
fn config_label(explicit: Option<&std::path::Path>) -> Result<String> {
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
