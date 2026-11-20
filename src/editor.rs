use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::EditorProfile;

/// Replace `{file}` with the absolute path and `{line}` with the line
/// number (or `1`) in every argument.
pub fn substitute(args: &[String], file: &Path, line: Option<u32>) -> Vec<String> {
    let file = file.display().to_string();
    let line = line.unwrap_or(1).to_string();
    args.iter()
        .map(|arg| arg.replace("{file}", &file).replace("{line}", &line))
        .collect()
}

/// Run the configured editor in the repository root, inheriting the
/// terminal. Returns an error when the command cannot start or exits
/// with a non-zero status.
pub fn launch(root: &Path, profile: &EditorProfile, file: &Path, line: Option<u32>) -> Result<()> {
    let args = substitute(&profile.command, file, line);
    let (program, rest) = args.split_first().context("editor command is empty")?;
    let status = Command::new(program)
        .args(rest)
        .current_dir(root)
        .status()
        .with_context(|| format!("cannot spawn editor `{program}`"))?;
    if !status.success() {
        bail!("editor `{program}` exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitute_replaces_file_and_line_in_every_argument() {
        let args = vec![
            "nvim".to_string(),
            "+{line}".to_string(),
            "{file}".to_string(),
            "--cmd".to_string(),
            "echo {file}:{line}".to_string(),
        ];
        assert_eq!(
            substitute(&args, Path::new("/repo/src/main.rs"), Some(42)),
            vec![
                "nvim",
                "+42",
                "/repo/src/main.rs",
                "--cmd",
                "echo /repo/src/main.rs:42",
            ]
        );
    }

    #[test]
    fn substitute_defaults_a_missing_line_to_one() {
        let args = vec!["hx".to_string(), "{file}:{line}".to_string()];
        assert_eq!(
            substitute(&args, Path::new("/repo/a.txt"), None),
            vec!["hx", "/repo/a.txt:1"]
        );
    }
}
