use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

pub mod files;
pub mod status;
pub mod diff;

/// Resolve the repository root containing `start`.
pub fn repo_root(start: &Path) -> Result<PathBuf> {
    let output = run(start, &["rev-parse", "--show-toplevel"])?;
    if !output.status.success() {
        anyhow::bail!(
            "git rev-parse failed in `{}`: {}",
            start.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    while root.ends_with('/') {
        root.pop();
    }
    Ok(PathBuf::from(root))
}

/// Run `git --no-optional-locks -C <root> <args>` with the global env.
pub fn run(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    let mut cmd = Command::new("git");
    cmd.arg("--no-optional-locks").arg("-C").arg(root);
    cmd.args(args);
    cmd.env("LC_ALL", "C");
    cmd.env("GIT_PAGER", "cat");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.output()
        .with_context(|| format!("cannot spawn git in `{}`", root.display()))
}

/// Split a `-z` output into its NUL-separated records, dropping the trailing empty one.
pub fn split_z(buf: &[u8]) -> Vec<&[u8]> {
    let mut records = Vec::new();
    let mut start = 0;
    for (i, byte) in buf.iter().enumerate() {
        if *byte == 0 {
            records.push(&buf[start..i]);
            start = i + 1;
        }
    }
    if start < buf.len() {
        records.push(&buf[start..]);
    }
    records
}

/// Build a `PathBuf` from raw bytes, preserving non-UTF8 names on Unix.
pub fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).to_string())
    }
}
