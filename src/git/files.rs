use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::status::GitState;
use super::{path_from_bytes, run, split_z};

/// Tracked + untracked non-ignored repo-relative paths, sorted and deduplicated.
/// Paths from the status map are unioned in to cover staged deletions
/// that no longer appear in the index.
pub fn visible(root: &Path, status: &GitState) -> Result<Vec<PathBuf>> {
    let out = run(root, &["ls-files", "-z", "-c", "-o", "--exclude-standard"])
        .context("git ls-files failed to run")?;
    if !out.status.success() {
        bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let mut set: BTreeSet<PathBuf> = BTreeSet::new();
    for record in split_z(&out.stdout) {
        if record.is_empty() {
            continue;
        }
        set.insert(path_from_bytes(record));
    }
    for path in status.files.keys() {
        set.insert(path.clone());
    }
    Ok(set.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::status::{FileStatus, StatusKind};
    use std::collections::BTreeMap;

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("--no-optional-locks")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("LC_ALL", "C")
            .env("GIT_PAGER", "cat")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn status_entry(path: &str, kind: StatusKind, index: char, worktree: char) -> FileStatus {
        FileStatus {
            path: PathBuf::from(path),
            index,
            worktree,
            kind,
            orig_path: None,
            staged: index != '.',
        }
    }

    #[test]
    fn visible_merges_ls_files_and_status_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        std::fs::write(root.join("b.txt"), "b\n").unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("c.txt"), "c\n").unwrap();
        git(root, &["add", "a.txt", "b.txt", "sub/c.txt"]);
        git(
            root,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.com",
                "commit",
                "-q",
                "-m",
                "init",
            ],
        );
        // An untracked file appears via ls-files -o; a staged deletion does
        // not appear anywhere and must come in through the status union.
        std::fs::write(root.join("new.txt"), "n\n").unwrap();
        std::fs::remove_file(root.join("b.txt")).unwrap();

        let mut files = BTreeMap::new();
        files.insert(
            PathBuf::from("b.txt"),
            status_entry("b.txt", StatusKind::Deleted, 'D', '.'),
        );
        let state = GitState {
            branch: Some("main".to_string()),
            files,
            additions: 0,
            deletions: 0,
        };

        let result = visible(root, &state).unwrap();
        assert_eq!(
            result,
            vec![
                PathBuf::from("a.txt"),
                PathBuf::from("b.txt"),
                PathBuf::from("new.txt"),
                PathBuf::from("sub/c.txt"),
            ]
        );
    }
}
