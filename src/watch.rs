use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::app::AppEvent;
use crate::config::Config;

/// Paths that would only cause noise: everything under `.git`, anything
/// inside a configured ignored directory, `.DS_Store`, and `*.swp` files.
pub fn should_ignore(path: &Path, root: &Path, ignore_dirs: &[String]) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);

    if relative
        .components()
        .any(|component| component.as_os_str() == ".git")
    {
        return true;
    }

    if let Some(first) = relative.components().next() {
        if let Some(name) = first.as_os_str().to_str() {
            if ignore_dirs.iter().any(|dir| dir == name) {
                return true;
            }
        }
    }

    if path.file_name().map(|name| name == ".DS_Store").unwrap_or(false) {
        return true;
    }

    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.ends_with(".swp"))
        .unwrap_or(false)
}

/// Watch `root` recursively and deliver debounced `AppEvent::FsChanged`
/// batches with absolute paths. The returned watcher must stay alive for
/// as long as events are wanted.
pub fn spawn(
    root: &Path,
    config: &Config,
    tx: mpsc::Sender<AppEvent>,
) -> Result<RecommendedWatcher> {
    let debounce = Duration::from_millis(config.refresh_debounce_ms);
    let ignore_dirs = config.ignore_dirs.clone();
    // Backends may report canonical paths (macOS resolves /var to
    // /private/var), so filter against the canonical root when possible.
    let watched_root = match root.canonicalize() {
        Ok(canonical) => canonical,
        Err(_) => root.to_path_buf(),
    };

    let (batch_tx, batch_rx) = mpsc::channel::<Vec<PathBuf>>();

    // Coalesce bursts: collect until no event has arrived for `debounce`,
    // then deduplicate and hand one batch to the UI.
    let event_tx = tx.clone();
    thread::spawn(move || loop {
        let mut pending = match batch_rx.recv() {
            Ok(paths) => paths,
            Err(_) => break,
        };
        let mut deadline = Instant::now() + debounce;
        let mut connected = true;
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match batch_rx.recv_timeout(deadline - now) {
                Ok(paths) => {
                    pending.extend(paths);
                    deadline = Instant::now() + debounce;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    connected = false;
                    break;
                }
            }
        }
        let mut seen = BTreeSet::new();
        pending.retain(|path| seen.insert(path.clone()));
        if !pending.is_empty() && event_tx.send(AppEvent::FsChanged(pending)).is_err() {
            break;
        }
        if !connected {
            break;
        }
    });

    let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
        match result {
            Ok(event) => {
                if matches!(event.kind, notify::EventKind::Access(_)) {
                    return;
                }
                let paths: Vec<PathBuf> = event
                    .paths
                    .into_iter()
                    .filter(|path| !should_ignore(path, &watched_root, &ignore_dirs))
                    .collect();
                if !paths.is_empty() {
                    let _ = batch_tx.send(paths);
                }
            }
            Err(err) => {
                let _ = tx.send(AppEvent::Error(format!("watcher error: {err}")));
            }
        }
    })
    .context("cannot create filesystem watcher")?;

    watcher
        .watch(root, RecursiveMode::Recursive)
        .with_context(|| format!("cannot watch `{}`", root.display()))?;

    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ignore_dirs() -> Vec<String> {
        vec![
            "target".to_string(),
            "node_modules".to_string(),
            ".git".to_string(),
        ]
    }

    #[test]
    fn should_ignore_filters_expected_paths() {
        let root = Path::new("/repo");
        let ignored = ignore_dirs();

        assert!(should_ignore(Path::new("/repo/.git/index"), root, &ignored));
        assert!(should_ignore(Path::new("/repo/target/debug/x"), root, &ignored));
        assert!(should_ignore(Path::new("/repo/node_modules/a"), root, &ignored));
        assert!(should_ignore(Path::new("/repo/.DS_Store"), root, &ignored));
        assert!(should_ignore(Path::new("/repo/src/main.rs.swp"), root, &ignored));
        assert!(!should_ignore(Path::new("/repo/src/main.rs"), root, &ignored));
    }

    #[test]
    fn spawn_delivers_debounced_fs_changed_events() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let (tx, rx) = mpsc::channel();
        let mut config = Config::default();
        config.refresh_debounce_ms = 50;
        let _watcher = spawn(&root, &config, tx).unwrap();

        std::fs::write(root.join("watched.txt"), "hello\n").unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining) {
                Ok(AppEvent::FsChanged(paths)) => {
                    if let Some(path) = paths.iter().find(|path| path.ends_with("watched.txt")) {
                        let canonical_root = root.canonicalize().unwrap();
                        assert!(
                            path.starts_with(&root) || path.starts_with(&canonical_root),
                            "event path {} must live under {}",
                            path.display(),
                            root.display()
                        );
                        break;
                    }
                }
                Ok(_) => continue,
                Err(err) => panic!("no FsChanged event: {err}"),
            }
        }
    }
}
