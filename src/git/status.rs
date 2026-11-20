use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::{path_from_bytes, run, split_z};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
    Untracked,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    pub path: PathBuf,  // repo-relative
    pub index: char,    // '.', 'M', 'A', 'D', 'R', 'C', 'U'
    pub worktree: char, // '.', 'M', 'D', 'U'
    pub kind: StatusKind,
    pub orig_path: Option<PathBuf>,
    pub staged: bool, // index != '.'
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitState {
    pub branch: Option<String>,
    pub files: BTreeMap<PathBuf, FileStatus>,
    pub additions: usize,
    pub deletions: usize,
}

/// Parse `status --porcelain=v2 -z --branch` output.
pub fn parse_status(input: &[u8]) -> Result<(Option<String>, BTreeMap<PathBuf, FileStatus>)> {
    let tokens = split_z(input);
    let mut branch: Option<String> = None;
    let mut files = BTreeMap::new();
    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i];
        i += 1;
        if token.is_empty() {
            continue;
        }
        match token[0] {
            b'#' => {
                let text = String::from_utf8_lossy(token);
                if let Some(name) = text.strip_prefix("# branch.head ") {
                    let name = name.trim();
                    branch = match name {
                        "" | "(detached)" | "(initial)" => None,
                        other => Some(other.to_string()),
                    };
                }
            }
            b'1' if token.len() > 1 && token[1] == b' ' => {
                let Some(f) = fields(token, 9) else { continue };
                let Some((index, worktree)) = xy(f[1]) else {
                    continue;
                };
                let path = path_from_bytes(f[8]);
                files.insert(
                    path.clone(),
                    FileStatus {
                        path,
                        index,
                        worktree,
                        kind: primary_kind(index, worktree),
                        orig_path: None,
                        staged: index != '.',
                    },
                );
            }
            b'2' if token.len() > 1 && token[1] == b' ' => {
                let Some(f) = fields(token, 10) else { continue };
                let Some((index, worktree)) = xy(f[1]) else {
                    continue;
                };
                if i >= tokens.len() {
                    continue;
                }
                let orig_path = path_from_bytes(tokens[i]);
                i += 1;
                let path = path_from_bytes(f[9]);
                files.insert(
                    path.clone(),
                    FileStatus {
                        path,
                        index,
                        worktree,
                        kind: primary_kind(index, worktree),
                        orig_path: Some(orig_path),
                        staged: index != '.',
                    },
                );
            }
            b'u' if token.len() > 1 && token[1] == b' ' => {
                let Some(f) = fields(token, 9) else { continue };
                let Some((index, worktree)) = xy(f[1]) else {
                    continue;
                };
                let path = path_from_bytes(f[8]);
                files.insert(
                    path.clone(),
                    FileStatus {
                        path,
                        index,
                        worktree,
                        kind: StatusKind::Unmerged,
                        orig_path: None,
                        staged: index != '.',
                    },
                );
            }
            b'?' if token.len() > 1 && token[1] == b' ' => {
                let path = path_from_bytes(&token[2..]);
                files.insert(
                    path.clone(),
                    FileStatus {
                        path,
                        index: '?',
                        worktree: '.',
                        kind: StatusKind::Untracked,
                        orig_path: None,
                        staged: false,
                    },
                );
            }
            b'!' => {} // ignored
            _ => {}    // unknown records are skipped
        }
    }
    Ok((branch, files))
}

/// Read the full state: status, plus additions/deletions vs `HEAD`.
pub fn read(root: &Path) -> Result<GitState> {
    let out = run(
        root,
        &[
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--no-ahead-behind",
            "--untracked-files=normal",
        ],
    )
    .context("git status failed to run")?;
    if !out.status.success() {
        bail!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let (branch, files) = parse_status(&out.stdout)?;
    let (additions, deletions) = numstat(root)?;
    Ok(GitState {
        branch,
        files,
        additions,
        deletions,
    })
}

fn numstat(root: &Path) -> Result<(usize, usize)> {
    let head = run(root, &["rev-parse", "--verify", "--quiet", "HEAD"])?;
    if !head.status.success() {
        return Ok((0, 0));
    }
    let out = run(root, &["diff", "--numstat", "HEAD", "--"])?;
    if !out.status.success() {
        bail!(
            "git diff --numstat HEAD failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let mut additions = 0usize;
    let mut deletions = 0usize;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.split_whitespace();
        additions += parts.next().unwrap_or("-").parse::<usize>().unwrap_or(0);
        deletions += parts.next().unwrap_or("-").parse::<usize>().unwrap_or(0);
    }
    Ok((additions, deletions))
}

fn fields(token: &[u8], n: usize) -> Option<Vec<&[u8]>> {
    let parts: Vec<&[u8]> = token.splitn(n, |&b| b == b' ').collect();
    (parts.len() == n).then_some(parts)
}

fn xy(field: &[u8]) -> Option<(char, char)> {
    if field.len() != 2 {
        return None;
    }
    Some((field[0] as char, field[1] as char))
}

/// The primary kind comes from the worktree code if present, else the index code.
fn primary_kind(index: char, worktree: char) -> StatusKind {
    let code = if worktree != '.' { worktree } else { index };
    status_kind(code)
}

fn status_kind(code: char) -> StatusKind {
    match code {
        'M' => StatusKind::Modified,
        'A' => StatusKind::Added,
        'D' => StatusKind::Deleted,
        'R' => StatusKind::Renamed,
        'C' => StatusKind::Copied,
        'U' => StatusKind::Unmerged,
        '?' => StatusKind::Untracked,
        _ => StatusKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(parts: &[&[u8]]) -> Vec<u8> {
        let mut buf = Vec::new();
        for part in parts {
            buf.extend_from_slice(part);
            buf.push(0);
        }
        buf
    }

    #[test]
    fn ordinary_modified_worktree_code_drives_kind() {
        let input = bytes(&[b"1 .M N... 100644 100644 100644 0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 src/main.rs"]);
        let (branch, files) = parse_status(&input).unwrap();
        assert_eq!(branch, None);
        assert_eq!(files.len(), 1);
        let st = &files[Path::new("src/main.rs")];
        assert_eq!(st.index, '.');
        assert_eq!(st.worktree, 'M');
        assert_eq!(st.kind, StatusKind::Modified);
        assert_eq!(st.orig_path, None);
        assert!(!st.staged);
    }

    #[test]
    fn untracked_record() {
        let input = bytes(&[b"? new.txt"]);
        let (_branch, files) = parse_status(&input).unwrap();
        let st = &files[Path::new("new.txt")];
        assert_eq!(st.index, '?');
        assert_eq!(st.worktree, '.');
        assert_eq!(st.kind, StatusKind::Untracked);
        assert!(!st.staged);
    }

    #[test]
    fn staged_addition() {
        let input = bytes(&[b"1 A. N... 0000000 100644 100644 0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 lib.rs"]);
        let (_branch, files) = parse_status(&input).unwrap();
        let st = &files[Path::new("lib.rs")];
        assert_eq!(st.index, 'A');
        assert_eq!(st.worktree, '.');
        assert_eq!(st.kind, StatusKind::Added);
        assert!(st.staged);
    }

    #[test]
    fn rename_record_consumes_next_nul_token_as_orig_path() {
        let input = bytes(&[
            b"2 R. N... 100644 100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 R100 sub/new.txt",
            b"sub/old.txt",
        ]);
        let (_branch, files) = parse_status(&input).unwrap();
        assert_eq!(files.len(), 1);
        let st = &files[Path::new("sub/new.txt")];
        assert_eq!(st.kind, StatusKind::Renamed);
        assert_eq!(st.orig_path.as_deref(), Some(Path::new("sub/old.txt")));
        assert!(st.staged);
    }

    #[test]
    fn unmerged_record() {
        let input = bytes(&[b"u UU N... 100644 100644 100644 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 conflict.txt"]);
        let (_branch, files) = parse_status(&input).unwrap();
        let st = &files[Path::new("conflict.txt")];
        assert_eq!(st.kind, StatusKind::Unmerged);
        assert_eq!(st.index, 'U');
        assert_eq!(st.worktree, 'U');
    }

    #[test]
    fn branch_header_parses_name_and_detached() {
        let input = bytes(&[
            b"# branch.oid 05d0c559fb8d7a7066c47650114c9d4d020ed58e",
            b"# branch.head feature/xyz",
            b"# branch.upstream origin/feature/xyz",
            b"# branch.ab +2 -3",
        ]);
        let (branch, files) = parse_status(&input).unwrap();
        assert_eq!(branch.as_deref(), Some("feature/xyz"));
        assert!(files.is_empty());

        let detached = bytes(&[b"# branch.head (detached)"]);
        let (branch, _files) = parse_status(&detached).unwrap();
        assert_eq!(branch, None);
    }

    #[test]
    fn ignored_and_unknown_records_are_skipped() {
        let input = bytes(&[
            b"! ignored.txt",
            b"9 nonsense record",
            b"# branch.oid (initial)",
            b"1 M. N... 100644 100644 100644 0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 kept.txt",
        ]);
        let (_branch, files) = parse_status(&input).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files.contains_key(Path::new("kept.txt")));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_path_bytes_are_preserved() {
        use std::os::unix::ffi::OsStrExt;
        let mut record = b"1 M. N... 100644 100644 100644 0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 ".to_vec();
        record.extend_from_slice(b"\xff\xfe na\xc3\xafve.txt");
        let input = bytes(&[&record]);
        let (_branch, files) = parse_status(&input).unwrap();
        let expected =
            std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"\xff\xfe na\xc3\xafve.txt"));
        let st = files.get(&expected).expect("non-UTF8 path must be a key");
        assert_eq!(st.kind, StatusKind::Modified);
    }

    #[test]
    fn integration_read_and_visible_in_temp_repo() {
        use crate::git::files;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(
            root,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.com",
                "add",
                "a.txt",
            ],
        );
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
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(root.join("b.txt"), "untracked\n").unwrap();

        let state = read(root).unwrap();
        let a = state
            .files
            .get(Path::new("a.txt"))
            .expect("a.txt must be seen");
        assert_eq!(a.kind, StatusKind::Modified);
        assert!(!a.staged);
        let b = state
            .files
            .get(Path::new("b.txt"))
            .expect("b.txt must be seen");
        assert_eq!(b.kind, StatusKind::Untracked);
        assert_eq!((state.additions, state.deletions), (1, 0));

        let visible = files::visible(root, &state).unwrap();
        assert!(visible.contains(&PathBuf::from("a.txt")));
        assert!(visible.contains(&PathBuf::from("b.txt")));
    }

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
}
