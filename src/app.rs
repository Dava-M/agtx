use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::config::{Config, EditorProfile};
use crate::git::{files, status};
use crate::git::status::GitState;
use crate::tree::{build, Row, Tree};

/// Which pane owns the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Diff,
}

/// Events driving the app: keyboard, terminal resize, filesystem changes,
/// the once-per-second tick, and background errors.
#[derive(Debug, Clone)]
pub enum AppEvent {
    Key(KeyEvent),
    Resize,
    #[allow(dead_code)] // constructed by the filesystem watcher (Task 5)
    FsChanged(Vec<PathBuf>),
    Tick,
    #[allow(dead_code)] // constructed by the watcher thread on failure (Task 5)
    Error(String),
}

/// Files keep the `●` marker for this long after a filesystem event.
const RECENT_SECS: u64 = 10;

pub struct App {
    root: PathBuf,
    config: Config,
    git: GitState,
    tree: Tree,
    selected_row: usize,
    focus: Focus,
    changed_only: bool,
    show_staged: bool,
    status: String,
    recent: Vec<(PathBuf, Instant)>,
    pub dirty: bool,
    quit: bool,
}

impl App {
    pub fn new(root: PathBuf, config: Config, git: GitState, paths: Vec<PathBuf>) -> App {
        let tree = build(paths, &git.files);
        App {
            root,
            config,
            git,
            tree,
            selected_row: 0,
            focus: Focus::Tree,
            changed_only: false,
            show_staged: false,
            status: String::new(),
            recent: Vec::new(),
            dirty: true,
            quit: false,
        }
    }

    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key(key) => {
                if let Err(err) = self.on_key(key) {
                    self.status = format!("error: {err:#}");
                    self.dirty = true;
                }
            }
            AppEvent::Resize => self.dirty = true,
            AppEvent::FsChanged(paths) => {
                self.mark_recent(&paths, Instant::now());
                self.status = match self.refresh_git() {
                    Ok(()) => "updated".to_string(),
                    Err(err) => format!("error: {err:#}"),
                };
                self.dirty = true;
            }
            AppEvent::Tick => self.prune_recent(Instant::now()),
            AppEvent::Error(msg) => {
                self.status = msg;
                self.dirty = true;
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind == KeyEventKind::Release {
            return Ok(());
        }
        if self.focus == Focus::Diff {
            // Task 4 owns diff keys; Enter/Tab/Esc hand focus back to the tree.
            match key.code {
                KeyCode::Enter | KeyCode::Tab | KeyCode::Esc => {
                    self.focus = Focus::Tree;
                    self.dirty = true;
                }
                _ => {}
            }
            return Ok(());
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('g') => self.select_first(),
            KeyCode::Char('G') => self.select_last(),
            KeyCode::Enter => self.open_selected()?,
            KeyCode::Char(' ') => self.toggle_selected_dir(),
            KeyCode::Tab => {
                self.focus = Focus::Diff;
                self.dirty = true;
            }
            KeyCode::Esc => {
                self.focus = Focus::Tree;
                self.dirty = true;
            }
            KeyCode::Char('s') => {
                self.show_staged = !self.show_staged;
                self.status = if self.show_staged {
                    "showing staged changes".to_string()
                } else {
                    "showing all changes".to_string()
                };
                self.dirty = true;
            }
            KeyCode::Char('f') => {
                self.changed_only = !self.changed_only;
                self.tree.set_changed_only(self.changed_only);
                self.clamp_selection();
                self.status = if self.changed_only {
                    "showing changed files".to_string()
                } else {
                    "showing all files".to_string()
                };
                self.dirty = true;
            }
            KeyCode::Char('r') => {
                self.status = match self.refresh_git() {
                    Ok(()) => "updated".to_string(),
                    Err(err) => format!("error: {err:#}"),
                };
                self.dirty = true;
            }
            KeyCode::Char('e') => {
                self.status = match self.edit_target() {
                    Some((profile, path)) => format!(
                        "editor wired in Task 5: {} {}",
                        profile.command.join(" "),
                        path.display()
                    ),
                    None => "editor wired in Task 5 (select a file)".to_string(),
                };
                self.dirty = true;
            }
            _ => {}
        }
        Ok(())
    }

    /// Re-read git state and rebuild the tree. Collapse state is keyed by
    /// path, so it survives the rebuild; the selected path is restored and
    /// the selection clamped.
    pub fn refresh_git(&mut self) -> Result<()> {
        let state = status::read(&self.root)?;
        let paths = files::visible(&self.root, &state)?;
        let selected_path = self
            .tree
            .rows()
            .get(self.selected_row)
            .map(|row| row.path.clone());
        self.git = state;
        self.tree = build(paths, &self.git.files);
        self.tree.set_changed_only(self.changed_only);
        self.selected_row = match selected_path {
            Some(path) => self.tree.find_row(&path).unwrap_or(self.selected_row),
            None => self.selected_row,
        };
        self.clamp_selection();
        self.dirty = true;
        Ok(())
    }

    pub fn open_selected_diff(&mut self) -> Result<()> {
        self.status = "diff pane not implemented yet (Task 4)".to_string();
        self.dirty = true;
        Ok(())
    }

    #[allow(dead_code)] // wired to the diff pane in Task 4
    pub fn reload_diff(&mut self) -> Result<()> {
        self.status = "diff pane not implemented yet (Task 4)".to_string();
        self.dirty = true;
        Ok(())
    }

    pub fn edit_target(&self) -> Option<(EditorProfile, PathBuf)> {
        let row = self.tree.rows().get(self.selected_row)?;
        if row.is_dir {
            return None;
        }
        let profile = self.config.editor.profiles.get(&self.config.editor.default)?.clone();
        Some((profile, self.root.join(&row.path)))
    }

    pub fn mark_recent(&mut self, paths: &[PathBuf], now: Instant) {
        for path in paths {
            match self.recent.iter_mut().find(|(p, _)| p == path) {
                Some(entry) => entry.1 = now,
                None => self.recent.push((path.clone(), now)),
            }
        }
        if !paths.is_empty() {
            self.dirty = true;
        }
    }

    pub fn prune_recent(&mut self, now: Instant) {
        let before = self.recent.len();
        self.recent
            .retain(|(_, at)| now.duration_since(*at) < Duration::from_secs(RECENT_SECS));
        if self.recent.len() != before {
            self.dirty = true;
        }
    }

    pub fn is_recent(&self, path: &Path) -> bool {
        self.recent.iter().any(|(p, _)| p == path)
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    pub fn rows(&self) -> &[Row] {
        self.tree.rows()
    }

    pub fn selected_row(&self) -> usize {
        self.selected_row
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn git(&self) -> &GitState {
        &self.git
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn status_message(&self) -> &str {
        &self.status
    }

    fn open_selected(&mut self) -> Result<()> {
        match self.tree.rows().get(self.selected_row) {
            Some(row) if row.is_dir => {
                self.toggle_selected_dir();
                Ok(())
            }
            Some(_) => self.open_selected_diff(),
            None => Ok(()),
        }
    }

    fn toggle_selected_dir(&mut self) {
        let path = match self.tree.rows().get(self.selected_row) {
            Some(row) if row.is_dir => row.path.clone(),
            _ => return,
        };
        self.tree.toggle_collapse(self.selected_row);
        if let Some(idx) = self.tree.find_row(&path) {
            self.selected_row = idx;
        }
        self.clamp_selection();
        self.dirty = true;
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.tree.rows().len();
        if len == 0 {
            return;
        }
        let current = self.selected_row.min(len - 1);
        let next = (current as isize + delta).clamp(0, (len - 1) as isize) as usize;
        if next != current {
            self.selected_row = next;
            self.dirty = true;
        }
    }

    fn select_first(&mut self) {
        if !self.tree.rows().is_empty() && self.selected_row != 0 {
            self.selected_row = 0;
            self.dirty = true;
        }
    }

    fn select_last(&mut self) {
        let len = self.tree.rows().len();
        if len > 0 && self.selected_row != len - 1 {
            self.selected_row = len - 1;
            self.dirty = true;
        }
    }

    fn clamp_selection(&mut self) {
        let len = self.tree.rows().len();
        self.selected_row = self.selected_row.min(len.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::git::status::{FileStatus, StatusKind};

    fn key(code: KeyCode) -> AppEvent {
        AppEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn test_app() -> App {
        let mut files = BTreeMap::new();
        files.insert(
            PathBuf::from("src/lib.rs"),
            FileStatus {
                path: PathBuf::from("src/lib.rs"),
                index: '.',
                worktree: 'M',
                kind: StatusKind::Modified,
                orig_path: None,
                staged: false,
            },
        );
        let git = GitState {
            branch: Some("main".to_string()),
            files,
            additions: 3,
            deletions: 1,
        };
        App::new(
            PathBuf::from("/repo"),
            Config::default(),
            git,
            vec![
                PathBuf::from("a.txt"),
                PathBuf::from("src/lib.rs"),
                PathBuf::from("src/util/mod.rs"),
            ],
        )
    }

    fn selected_path(app: &App) -> Option<PathBuf> {
        app.rows()
            .get(app.selected_row())
            .map(|row| row.path.clone())
    }

    fn row_paths(app: &App) -> Vec<String> {
        app.rows()
            .iter()
            .map(|row| row.path.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn navigation_moves_and_clamps_at_both_ends() {
        let mut app = test_app();
        assert_eq!(selected_path(&app), Some(PathBuf::from("src")));

        app.handle_event(key(KeyCode::Char('j')));
        assert_eq!(selected_path(&app), Some(PathBuf::from("src/util")));
        app.handle_event(key(KeyCode::Down));
        assert_eq!(selected_path(&app), Some(PathBuf::from("src/util/mod.rs")));
        app.handle_event(key(KeyCode::Char('k')));
        assert_eq!(selected_path(&app), Some(PathBuf::from("src/util")));
        app.handle_event(key(KeyCode::Up));
        assert_eq!(selected_path(&app), Some(PathBuf::from("src")));

        app.handle_event(key(KeyCode::Char('g')));
        assert_eq!(selected_path(&app), Some(PathBuf::from("src")));
        app.handle_event(key(KeyCode::Char('k')));
        assert_eq!(selected_path(&app), Some(PathBuf::from("src")), "clamped at top");

        app.handle_event(key(KeyCode::Char('G')));
        assert_eq!(selected_path(&app), Some(PathBuf::from("a.txt")));
        for _ in 0..10 {
            app.handle_event(key(KeyCode::Char('j')));
        }
        assert_eq!(selected_path(&app), Some(PathBuf::from("a.txt")), "clamped at bottom");
    }

    #[test]
    fn enter_collapses_and_restores_directory() {
        let mut app = test_app();
        app.handle_event(key(KeyCode::Enter));
        assert_eq!(row_paths(&app), vec!["src", "a.txt"]);
        assert_eq!(selected_path(&app), Some(PathBuf::from("src")));

        app.handle_event(key(KeyCode::Enter));
        assert_eq!(
            row_paths(&app),
            vec!["src", "src/util", "src/util/mod.rs", "src/lib.rs", "a.txt"]
        );
    }

    #[test]
    fn space_toggles_directory_collapse() {
        let mut app = test_app();
        app.handle_event(key(KeyCode::Char(' ')));
        assert_eq!(row_paths(&app), vec!["src", "a.txt"]);
        app.handle_event(key(KeyCode::Char(' ')));
        assert_eq!(row_paths(&app).len(), 5);
    }

    #[test]
    fn changed_only_hides_unchanged_files_and_rebuilds_rows() {
        let mut app = test_app();
        app.handle_event(key(KeyCode::Char('f')));
        assert_eq!(row_paths(&app), vec!["src", "src/lib.rs"]);
        app.handle_event(key(KeyCode::Char('f')));
        assert_eq!(row_paths(&app).len(), 5);
    }

    #[test]
    fn enter_on_file_reports_not_implemented() {
        let mut app = test_app();
        app.handle_event(key(KeyCode::Char('j')));
        app.handle_event(key(KeyCode::Char('j')));
        app.handle_event(key(KeyCode::Char('j')));
        assert_eq!(selected_path(&app), Some(PathBuf::from("src/lib.rs")));
        app.handle_event(key(KeyCode::Enter));
        assert!(app.status_message().contains("not implemented"));
    }

    #[test]
    fn tab_and_esc_switch_focus() {
        let mut app = test_app();
        assert_eq!(app.focus(), Focus::Tree);
        app.handle_event(key(KeyCode::Tab));
        assert_eq!(app.focus(), Focus::Diff);
        app.handle_event(key(KeyCode::Esc));
        assert_eq!(app.focus(), Focus::Tree);
    }

    #[test]
    fn quit_keys_set_should_quit() {
        let mut app = test_app();
        app.handle_event(key(KeyCode::Char('q')));
        assert!(app.should_quit());

        let mut app = test_app();
        app.handle_event(AppEvent::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(app.should_quit());
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
    fn refresh_git_preserves_selection_and_picks_up_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        git(&root, &["init", "-q"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        std::fs::create_dir_all(root.join("b")).unwrap();
        std::fs::write(root.join("b").join("c.txt"), "c\n").unwrap();
        std::fs::write(root.join("b").join("d.txt"), "d\n").unwrap();
        git(&root, &["add", "a.txt", "b/c.txt", "b/d.txt"]);
        git(
            &root,
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

        let state = status::read(&root).unwrap();
        let paths = files::visible(&root, &state).unwrap();
        let mut app = App::new(root.clone(), Config::default(), state, paths);
        // rows: b, b/c.txt, b/d.txt, a.txt — select b/d.txt
        app.handle_event(key(KeyCode::Char('j')));
        app.handle_event(key(KeyCode::Char('j')));
        assert_eq!(selected_path(&app), Some(PathBuf::from("b/d.txt")));

        // The agent modifies a.txt and adds b/e.txt while agtx runs.
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(root.join("b").join("e.txt"), "e\n").unwrap();
        app.refresh_git().unwrap();

        assert_eq!(selected_path(&app), Some(PathBuf::from("b/d.txt")), "selection preserved");
        let paths = row_paths(&app);
        assert!(paths.contains(&"b/e.txt".to_string()), "new file picked up: {paths:?}");
        let changed: Vec<_> = app
            .rows()
            .iter()
            .filter_map(|row| {
                row.status
                    .as_ref()
                    .map(|status| (row.path.to_string_lossy().to_string(), status.kind))
            })
            .collect();
        assert_eq!(
            changed,
            vec![
                ("b/e.txt".to_string(), StatusKind::Untracked),
                ("a.txt".to_string(), StatusKind::Modified),
            ]
        );
        assert!(app.dirty);
    }
}
