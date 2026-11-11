use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::run;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Header,
    Hunk,
    Context,
    Add,
    Del,
    Meta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffView {
    pub path: PathBuf,
    pub staged: bool,
    pub lines: Vec<DiffLine>,
    pub truncated: bool,
    pub binary: bool,
}

/// Load the uncommitted diff for `rel` (repo-relative).
///
/// Command choice: untracked files diff against `/dev/null` via
/// `--no-index`; staged files use `--cached`; everything else diffs
/// against `HEAD`, falling back to `--cached` when `HEAD` is missing.
pub fn load(root: &Path, rel: &Path, staged: bool, max_bytes: usize, max_lines: usize) -> Result<DiffView> {
    let rel_str = rel.to_string_lossy();
    let probe = run(root, &["ls-files", "--error-unmatch", "--", &rel_str])
        .with_context(|| format!("cannot probe `{}` in the index", rel.display()))?;
    let in_index = probe.status.success();
    let on_disk = root.join(rel).exists();
    let untracked = !in_index && on_disk;

    let (args, no_index) = if untracked {
        (
            vec![
                "diff",
                "--no-index",
                "--no-color",
                "--",
                "/dev/null",
                &rel_str,
            ],
            true,
        )
    } else if staged {
        (
            vec![
                "diff",
                "--cached",
                "--no-color",
                "--no-ext-diff",
                "-U3",
                "--",
                &rel_str,
            ],
            false,
        )
    } else if head_exists(root)? {
        (
            vec![
                "diff",
                "HEAD",
                "--no-color",
                "--no-ext-diff",
                "-U3",
                "--",
                &rel_str,
            ],
            false,
        )
    } else {
        (
            vec![
                "diff",
                "--cached",
                "--no-color",
                "--no-ext-diff",
                "-U3",
                "--",
                &rel_str,
            ],
            false,
        )
    };

    let out = run(root, &args)
        .with_context(|| format!("git diff failed to run for `{}`", rel.display()))?;
    let code = out.status.code().unwrap_or(-1);
    if no_index {
        // `--no-index` exits 1 when the files differ; that is success here.
        if code != 0 && code != 1 {
            bail!(
                "git diff --no-index failed for `{}` (exit {code}): {}",
                rel.display(),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    } else if code != 0 {
        bail!(
            "git diff failed for `{}` (exit {code}): {}",
            rel.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let mut truncated = false;
    let bytes: &[u8] = if out.stdout.len() > max_bytes {
        truncated = true;
        &out.stdout[..max_bytes]
    } else {
        &out.stdout
    };
    let text = String::from_utf8_lossy(bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if lines.len() > max_lines {
        truncated = true;
        lines.truncate(max_lines);
    }
    let text = lines.join("\n");
    Ok(parse(&text, rel.to_path_buf(), staged, truncated))
}

fn head_exists(root: &Path) -> Result<bool> {
    let out = run(root, &["rev-parse", "--verify", "--quiet", "HEAD"])?;
    Ok(out.status.success())
}

/// Parse unified diff text into a `DiffView`.
pub fn parse(text: &str, path: PathBuf, staged: bool, truncated: bool) -> DiffView {
    let mut lines = Vec::new();
    let mut binary = false;
    let mut in_hunk = false;
    let mut old_no = 0u32;
    let mut new_no = 0u32;
    for raw in text.lines() {
        if (raw.starts_with("Binary files ") && raw.ends_with(" differ")) || raw == "GIT binary patch" {
            binary = true;
            lines.push(DiffLine {
                kind: DiffLineKind::Meta,
                text: raw.to_string(),
                old_no: None,
                new_no: None,
            });
            continue;
        }
        if let Some((a, c)) = raw.strip_prefix("@@ ").and_then(parse_hunk_start) {
            old_no = a;
            new_no = c;
            in_hunk = true;
            lines.push(DiffLine {
                kind: DiffLineKind::Hunk,
                text: raw.to_string(),
                old_no: Some(a),
                new_no: Some(c),
            });
            continue;
        }
        if !in_hunk {
            let kind = if raw.starts_with("diff --")
                || raw.starts_with("index ")
                || raw.starts_with("--- ")
                || raw.starts_with("+++ ")
            {
                DiffLineKind::Header
            } else {
                DiffLineKind::Meta
            };
            lines.push(DiffLine {
                kind,
                text: raw.to_string(),
                old_no: None,
                new_no: None,
            });
            continue;
        }
        let line = match raw.as_bytes().first() {
            Some(b' ') => {
                let line = DiffLine {
                    kind: DiffLineKind::Context,
                    text: raw.to_string(),
                    old_no: Some(old_no),
                    new_no: Some(new_no),
                };
                old_no += 1;
                new_no += 1;
                line
            }
            Some(b'+') => {
                let line = DiffLine {
                    kind: DiffLineKind::Add,
                    text: raw.to_string(),
                    old_no: None,
                    new_no: Some(new_no),
                };
                new_no += 1;
                line
            }
            Some(b'-') => {
                let line = DiffLine {
                    kind: DiffLineKind::Del,
                    text: raw.to_string(),
                    old_no: Some(old_no),
                    new_no: None,
                };
                old_no += 1;
                line
            }
            _ => DiffLine {
                kind: DiffLineKind::Meta,
                text: raw.to_string(),
                old_no: None,
                new_no: None,
            },
        };
        lines.push(line);
    }
    DiffView {
        path,
        staged,
        lines,
        truncated,
        binary,
    }
}

/// `@@ -a[,b] +c[,d] @@ ...` -> `(a, c)`
fn parse_hunk_start(rest: &str) -> Option<(u32, u32)> {
    let rest = rest.strip_prefix('-')?;
    let (old_part, remainder) = rest.split_once(' ')?;
    let new_part = remainder.strip_prefix('+')?;
    let new_part = new_part.split(' ').next()?;
    let old_start = old_part.split(',').next()?.parse::<u32>().ok()?;
    let new_start = new_part.split(',').next()?.parse::<u32>().ok()?;
    Some((old_start, new_start))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parse_sample_with_hunk_and_line_numbers() {
        let text = [
            "diff --git a/src/main.rs b/src/main.rs",
            "index 1234567..89abcde 100644",
            "--- a/src/main.rs",
            "+++ b/src/main.rs",
            "@@ -1,4 +1,5 @@",
            " fn main() {",
            "-    println!(\"old\");",
            "+    println!(\"new\");",
            "+    println!(\"extra\");",
            " }",
        ]
        .join("\n");
        let view = parse(&text, PathBuf::from("src/main.rs"), false, false);
        assert_eq!(view.path, Path::new("src/main.rs"));
        assert!(!view.staged);
        assert!(!view.truncated);
        assert!(!view.binary);
        assert_eq!(view.lines.len(), 10);
        assert_eq!(view.lines[0].kind, DiffLineKind::Header);
        assert_eq!(view.lines[1].kind, DiffLineKind::Header);
        assert_eq!(view.lines[2].kind, DiffLineKind::Header);
        assert_eq!(view.lines[3].kind, DiffLineKind::Header);
        let hunk = &view.lines[4];
        assert_eq!(hunk.kind, DiffLineKind::Hunk);
        assert_eq!(hunk.old_no, Some(1));
        assert_eq!(hunk.new_no, Some(1));
        let ctx = &view.lines[5];
        assert_eq!(ctx.kind, DiffLineKind::Context);
        assert_eq!(ctx.old_no, Some(1));
        assert_eq!(ctx.new_no, Some(1));
        let del = &view.lines[6];
        assert_eq!(del.kind, DiffLineKind::Del);
        assert_eq!(del.old_no, Some(2));
        assert_eq!(del.new_no, None);
        let add1 = &view.lines[7];
        assert_eq!(add1.kind, DiffLineKind::Add);
        assert_eq!(add1.old_no, None);
        assert_eq!(add1.new_no, Some(2));
        let add2 = &view.lines[8];
        assert_eq!(add2.kind, DiffLineKind::Add);
        assert_eq!(add2.new_no, Some(3));
        let ctx2 = &view.lines[9];
        assert_eq!(ctx2.kind, DiffLineKind::Context);
        assert_eq!(ctx2.old_no, Some(3));
        assert_eq!(ctx2.new_no, Some(4));
    }

    #[test]
    fn parse_hunk_header_without_counts() {
        let text = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -3 +3 @@\n-x\n+y\n";
        let view = parse(text, PathBuf::from("f"), false, false);
        let hunk = &view.lines[3];
        assert_eq!(hunk.kind, DiffLineKind::Hunk);
        assert_eq!(hunk.old_no, Some(3));
        assert_eq!(hunk.new_no, Some(3));
        assert_eq!(view.lines[4].kind, DiffLineKind::Del);
        assert_eq!(view.lines[4].old_no, Some(3));
        assert_eq!(view.lines[5].kind, DiffLineKind::Add);
        assert_eq!(view.lines[5].new_no, Some(3));
    }

    #[test]
    fn binary_markers_set_binary_flag() {
        let text = "diff --git a/img.png b/img.png\nindex 111..222 100644\nBinary files a/img.png and b/img.png differ\n";
        let view = parse(text, PathBuf::from("img.png"), false, false);
        assert!(view.binary);
        assert_eq!(view.lines[2].kind, DiffLineKind::Meta);

        let text = "diff --git a/x b/x\nindex 111..222 100644\nGIT binary patch\nliteral 8\n";
        let view = parse(text, PathBuf::from("x"), false, false);
        assert!(view.binary);
        assert_eq!(view.lines[2].kind, DiffLineKind::Meta);
        assert_eq!(view.lines[3].kind, DiffLineKind::Meta);
    }

    #[test]
    fn no_newline_marker_is_meta() {
        let text = [
            "diff --git a/f b/f",
            "index 111..222 100644",
            "--- a/f",
            "+++ b/f",
            "@@ -1 +1 @@",
            "-old",
            "\\ No newline at end of file",
            "+new",
            "\\ No newline at end of file",
        ]
        .join("\n");
        let view = parse(&text, PathBuf::from("f"), false, false);
        assert_eq!(view.lines[4].kind, DiffLineKind::Hunk);
        assert_eq!(view.lines[5].kind, DiffLineKind::Del);
        assert_eq!(view.lines[5].old_no, Some(1));
        assert_eq!(view.lines[6].text, "\\ No newline at end of file");
        assert_eq!(view.lines[6].kind, DiffLineKind::Meta);
        assert_eq!(view.lines[7].kind, DiffLineKind::Add);
        assert_eq!(view.lines[7].new_no, Some(1));
        assert_eq!(view.lines[8].text, "\\ No newline at end of file");
        assert_eq!(view.lines[8].kind, DiffLineKind::Meta);
    }

    #[test]
    fn empty_diff_parses_to_nothing() {
        let view = parse("", PathBuf::from("f"), false, false);
        assert!(view.lines.is_empty());
        assert!(!view.binary);
        assert!(!view.truncated);
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

    #[test]
    fn load_picks_command_and_respects_limits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(root, &["add", "a.txt"]);
        git(
            root,
            &["-c", "user.name=T", "-c", "user.email=t@example.com", "commit", "-q", "-m", "init"],
        );
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(root.join("u.txt"), "untracked\n").unwrap();

        let view = load(root, Path::new("a.txt"), false, 2_000_000, 20_000).unwrap();
        assert!(!view.binary);
        assert!(!view.truncated);
        assert!(view
            .lines
            .iter()
            .any(|l| l.kind == DiffLineKind::Add && l.text.ends_with("two")));

        let view = load(root, Path::new("u.txt"), false, 2_000_000, 20_000).unwrap();
        assert!(view
            .lines
            .iter()
            .any(|l| l.kind == DiffLineKind::Add && l.text.ends_with("untracked")));

        git(root, &["add", "u.txt"]);
        let view = load(root, Path::new("u.txt"), true, 2_000_000, 20_000).unwrap();
        assert!(view
            .lines
            .iter()
            .any(|l| l.kind == DiffLineKind::Add && l.text.ends_with("untracked")));

        let view = load(root, Path::new("a.txt"), false, 4, 20_000).unwrap();
        assert!(view.truncated);

        let view = load(root, Path::new("a.txt"), false, 2_000_000, 1).unwrap();
        assert!(view.truncated);
        assert!(view.lines.len() <= 1);
    }
}
