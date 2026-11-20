use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::config::{Config, EditorProfile};
use crate::git::diff::{self, DiffView, ViewKind};
use crate::git::status::GitState;
use crate::git::{files, status};
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
    FsChanged(Vec<PathBuf>),
    Tick,
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
    diff: Option<DiffView>,
    diff_scroll: usize,
    /// Set by the diff pane during draw so paging matches the real viewport.
    diff_viewport_height: Cell<u16>,
    status: String,
    recent: Vec<(PathBuf, Instant)>,
    /// Set by `e`; taken by the main loop, which owns the terminal.
    editor_request: Option<(EditorProfile, PathBuf)>,
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
            diff: None,
            diff_scroll: 0,
            diff_viewport_height: Cell::new(20),
            status: String::new(),
            recent: Vec::new(),
            editor_request: None,
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
                let relative: Vec<PathBuf> = paths
                    .iter()
                    .filter_map(|path| self.repo_relative(path))
                    .collect();
                self.mark_recent(&relative, Instant::now());
                self.status = match self.refresh_git().and_then(|_| self.reload_diff()) {
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
            match key.code {
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.preview_current()?
                }
                KeyCode::Enter => self.queue_editor(),
                KeyCode::Tab | KeyCode::Esc => {
                    self.focus = Focus::Tree;
                    self.dirty = true;
                }
                KeyCode::Char('q') => self.quit = true,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.quit = true;
                }
                KeyCode::Char('j') | KeyCode::Down => self.scroll_diff(1),
                KeyCode::Char('k') | KeyCode::Up => self.scroll_diff(-1),
                KeyCode::PageDown => {
                    let page = self.diff_viewport_height().max(1) as isize;
                    self.scroll_diff(page);
                }
                KeyCode::PageUp => {
                    let page = self.diff_viewport_height().max(1) as isize;
                    self.scroll_diff(-page);
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.scroll_diff(10);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.scroll_diff(-10);
                }
                KeyCode::Home => self.scroll_diff_to(0),
                KeyCode::End => {
                    let end = self.diff_max_scroll();
                    self.scroll_diff_to(end);
                }
                KeyCode::Char('s') => self.toggle_staged(),
                KeyCode::Char('r') => self.refresh_and_reload(),
                KeyCode::Char('e') => self.queue_editor(),
                KeyCode::Char('p') => self.preview_current()?,
                KeyCode::Char('-') => self.collapse_to_current(),
                KeyCode::Char('=') => self.expand_all(),
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
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.open_selected_preview()?
            }
            KeyCode::Enter => self.open_selected()?,
            KeyCode::Char(' ') => self.toggle_selected_dir(),
            KeyCode::Char('p') => self.open_selected_preview()?,
            KeyCode::Char('-') => self.collapse_to_current(),
            KeyCode::Char('=') => self.expand_all(),
            KeyCode::Tab => {
                self.focus = Focus::Diff;
                self.dirty = true;
            }
            KeyCode::Esc => {
                self.focus = Focus::Tree;
                self.dirty = true;
            }
            KeyCode::Char('s') => self.toggle_staged(),
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
            KeyCode::Char('r') => self.refresh_and_reload(),
            KeyCode::Char('e') => self.queue_editor(),
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

    /// Load the diff for the selected file into the right pane.
    pub fn open_selected_diff(&mut self) -> Result<()> {
        let path = match self.tree.rows().get(self.selected_row) {
            Some(row) if !row.is_dir => row.path.clone(),
            _ => return Ok(()),
        };
        self.load_diff_for(path)?;
        self.focus = Focus::Diff;
        self.diff_scroll = 0;
        self.dirty = true;
        Ok(())
    }

    /// Load the file's current contents into the right pane, even when it
    /// has no uncommitted changes.
    pub fn open_selected_preview(&mut self) -> Result<()> {
        let path = match self.tree.rows().get(self.selected_row) {
            Some(row) if !row.is_dir => row.path.clone(),
            _ => {
                self.status = "select a file to preview".to_string();
                self.dirty = true;
                return Ok(());
            }
        };
        self.preview_for(path)?;
        self.focus = Focus::Diff;
        self.diff_scroll = 0;
        self.dirty = true;
        Ok(())
    }

    fn preview_for(&mut self, path: PathBuf) -> Result<()> {
        let view = diff::load_file(
            &self.root,
            &path,
            self.config.max_diff_bytes,
            self.config.max_diff_lines,
        )?;
        self.diff = Some(view);
        self.diff_scroll = self.diff_scroll.min(self.diff_max_scroll());
        self.status = "file preview".to_string();
        self.dirty = true;
        Ok(())
    }

    fn preview_current(&mut self) -> Result<()> {
        let path = match self.diff.as_ref() {
            Some(view) => view.path.clone(),
            None => {
                self.status = "no file to preview".to_string();
                self.dirty = true;
                return Ok(());
            }
        };
        self.preview_for(path)?;
        self.focus = Focus::Diff;
        self.diff_scroll = 0;
        Ok(())
    }

    /// Reload the open diff after a git refresh or a staged toggle.
    /// A closed diff pane is a no-op; a failed load keeps the previous view
    /// and reports the error to the caller.
    pub fn reload_diff(&mut self) -> Result<()> {
        let (path, kind) = match self.diff.as_ref() {
            Some(view) => (view.path.clone(), view.kind),
            None => return Ok(()),
        };
        match kind {
            ViewKind::File => self.preview_for(path),
            ViewKind::Diff => self.load_diff_for(path),
        }
    }

    fn load_diff_for(&mut self, path: PathBuf) -> Result<()> {
        let view = diff::load(
            &self.root,
            &path,
            self.show_staged,
            self.config.max_diff_bytes,
            self.config.max_diff_lines,
        )?;
        self.diff = Some(view);
        self.diff_scroll = self.diff_scroll.min(self.diff_max_scroll());
        self.dirty = true;
        Ok(())
    }

    fn toggle_staged(&mut self) {
        self.show_staged = !self.show_staged;
        let path = self.diff.as_ref().map(|view| view.path.clone());
        self.status = match path {
            Some(path) => match self.load_diff_for(path) {
                Ok(()) if self.show_staged => "showing staged changes".to_string(),
                Ok(()) => "showing all changes".to_string(),
                Err(err) => format!("error: {err:#}"),
            },
            None => {
                if self.show_staged {
                    "showing staged changes".to_string()
                } else {
                    "showing all changes".to_string()
                }
            }
        };
        self.dirty = true;
    }

    fn refresh_and_reload(&mut self) {
        self.status = match self.refresh_git().and_then(|_| self.reload_diff()) {
            Ok(()) => "updated".to_string(),
            Err(err) => format!("error: {err:#}"),
        };
        self.dirty = true;
    }

    fn diff_max_scroll(&self) -> usize {
        let len = self.diff.as_ref().map(|view| view.lines.len()).unwrap_or(0);
        len.saturating_sub(self.diff_viewport_height().max(1) as usize)
    }

    fn scroll_diff(&mut self, delta: isize) {
        let next = (self.diff_scroll as isize + delta).clamp(0, self.diff_max_scroll() as isize);
        self.scroll_diff_to(next as usize);
    }

    fn scroll_diff_to(&mut self, pos: usize) {
        let next = pos.min(self.diff_max_scroll());
        if next != self.diff_scroll {
            self.diff_scroll = next;
            self.dirty = true;
        }
    }

    pub fn edit_target(&self) -> Option<(EditorProfile, PathBuf)> {
        let relative = if self.focus == Focus::Diff {
            self.diff.as_ref()?.path.clone()
        } else {
            let row = self.tree.rows().get(self.selected_row)?;
            if row.is_dir {
                return None;
            }
            row.path.clone()
        };
        let profile = self
            .config
            .editor
            .profiles
            .get(&self.config.editor.default)?
            .clone();
        Some((profile, self.root.join(relative)))
    }

    /// Queue an editor launch for the selected file, or for the file whose
    /// diff is open when the diff pane has focus.
    fn queue_editor(&mut self) {
        match self.edit_target() {
            Some((profile, path)) => {
                self.editor_request = Some((profile, path));
                self.status.clear();
            }
            None => {
                self.status = if self.focus == Focus::Diff {
                    "no diff open".to_string()
                } else {
                    match self.tree.rows().get(self.selected_row) {
                        Some(row) if row.is_dir => {
                            "select a file to open in the editor".to_string()
                        }
                        Some(_) => format!(
                            "editor profile `{}` is not defined",
                            self.config.editor.default
                        ),
                        None => "no file selected".to_string(),
                    }
                };
            }
        }
        self.dirty = true;
    }

    /// Consume a pending editor launch request (set by the `e` key).
    pub fn take_editor_request(&mut self) -> Option<(EditorProfile, PathBuf)> {
        self.editor_request.take()
    }

    /// Report a message from the main loop, e.g. an editor launch failure.
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = message.into();
        self.dirty = true;
    }

    /// Convert a watcher path to repo-relative, dropping paths outside the
    /// root. Watcher backends may report canonical paths, so the canonical
    /// root is tried when the plain prefix does not match.
    fn repo_relative(&self, path: &Path) -> Option<PathBuf> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let relative = match absolute.strip_prefix(&self.root) {
            Ok(rel) => rel.to_path_buf(),
            Err(_) => {
                let canonical_root = self.root.canonicalize().ok()?;
                absolute.strip_prefix(&canonical_root).ok()?.to_path_buf()
            }
        };
        if relative.as_os_str().is_empty() {
            None
        } else {
            Some(relative)
        }
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

    pub fn diff(&self) -> Option<&DiffView> {
        self.diff.as_ref()
    }

    pub fn diff_scroll(&self) -> usize {
        self.diff_scroll
    }

    pub fn diff_viewport_height(&self) -> u16 {
        self.diff_viewport_height.get()
    }

    /// Called by the diff pane on every draw so paging follows the real height.
    pub fn set_diff_viewport_height(&self, height: u16) {
        self.diff_viewport_height.set(height.max(1));
    }

    /// Preload a parsed view; used by tests that cannot run git.
    #[cfg(test)]
    pub(crate) fn preload_diff(&mut self, view: DiffView) {
        self.diff = Some(view);
        self.focus = Focus::Diff;
        self.diff_scroll = 0;
        self.dirty = true;
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

    fn collapse_to_current(&mut self) {
        let path = if self.focus == Focus::Diff {
            self.diff.as_ref().map(|view| view.path.clone())
        } else {
            self.tree
                .rows()
                .get(self.selected_row)
                .map(|row| row.path.clone())
        };
        let Some(path) = path else {
            return;
        };
        self.tree.collapse_to(&path);
        self.selected_row = self.tree.find_row(&path).unwrap_or(0);
        self.clamp_selection();
        self.status = "collapsed to current directory".to_string();
        self.dirty = true;
    }

    fn expand_all(&mut self) {
        let path = self
            .tree
            .rows()
            .get(self.selected_row)
            .map(|row| row.path.clone());
        self.tree.expand_all();
        if let Some(path) = path {
            self.selected_row = self.tree.find_row(&path).unwrap_or(0);
        }
        self.clamp_selection();
        self.status = "expanded all".to_string();
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

    use crate::git::diff::DiffLineKind;
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
        assert_eq!(
            selected_path(&app),
            Some(PathBuf::from("src")),
            "clamped at top"
        );

        app.handle_event(key(KeyCode::Char('G')));
        assert_eq!(selected_path(&app), Some(PathBuf::from("a.txt")));
        for _ in 0..10 {
            app.handle_event(key(KeyCode::Char('j')));
        }
        assert_eq!(
            selected_path(&app),
            Some(PathBuf::from("a.txt")),
            "clamped at bottom"
        );
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

    /// A repo with a committed `a.txt`, a staged edit (`two`) and a further
    /// unstaged edit (`three`), so worktree and staged diffs differ.
    fn staged_and_worktree_changes() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        git(&root, &["init", "-q"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&root, &["add", "a.txt"]);
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
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        git(&root, &["add", "a.txt"]);
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        (dir, root)
    }

    fn app_for(root: &Path) -> App {
        let state = status::read(root).unwrap();
        let paths = files::visible(root, &state).unwrap();
        App::new(root.to_path_buf(), Config::default(), state, paths)
    }

    #[test]
    fn enter_on_file_opens_diff_and_focuses_the_pane() {
        let (_dir, root) = staged_and_worktree_changes();
        let mut app = app_for(&root);
        assert_eq!(selected_path(&app), Some(PathBuf::from("a.txt")));

        app.handle_event(key(KeyCode::Enter));

        assert_eq!(app.focus(), Focus::Diff);
        assert_eq!(app.diff_scroll(), 0);
        let view = app.diff().expect("diff must be loaded");
        assert_eq!(view.path, PathBuf::from("a.txt"));
        assert!(!view.staged);
        assert!(view
            .lines
            .iter()
            .any(|line| line.kind == DiffLineKind::Add && line.text.contains("three")));
        assert!(app.status_message().is_empty());
    }

    #[test]
    fn s_toggles_staged_and_reloads_the_open_diff() {
        let (_dir, root) = staged_and_worktree_changes();
        let mut app = app_for(&root);
        app.open_selected_diff().unwrap();
        assert!(!app.diff().unwrap().staged);
        assert!(app
            .diff()
            .unwrap()
            .lines
            .iter()
            .any(|l| l.text.contains("three")));

        app.handle_event(key(KeyCode::Char('s')));
        let view = app.diff().unwrap();
        assert!(view.staged, "diff reloaded with the staged flag");
        assert!(view.lines.iter().any(|l| l.text.contains("two")));
        assert!(
            !view.lines.iter().any(|l| l.text.contains("three")),
            "staged-only diff hides the worktree edit"
        );
        assert!(app.status_message().contains("staged"));

        app.handle_event(key(KeyCode::Char('s')));
        let view = app.diff().unwrap();
        assert!(!view.staged);
        assert!(view.lines.iter().any(|l| l.text.contains("three")));

        app.handle_event(key(KeyCode::Enter));
        assert_eq!(
            app.focus(),
            Focus::Diff,
            "Enter in the diff pane keeps focus"
        );
        let (profile, path) = app
            .take_editor_request()
            .expect("Enter in the diff pane queues the editor");
        assert_eq!(profile.command, vec!["nvim", "+{line}", "{file}"]);
        assert_eq!(path, root.join("a.txt"));

        app.handle_event(key(KeyCode::Tab));
        assert_eq!(app.focus(), Focus::Tree, "Tab returns to the tree");
    }

    #[test]
    fn diff_scrolling_clamps_to_both_ends() {
        let mut app = test_app();
        let body: Vec<String> = (1..=60).map(|i| format!("+line {i}")).collect();
        let text = format!("diff --git a/f b/f\n@@ -1 +1,60 @@\n{}", body.join("\n"));
        app.preload_diff(crate::git::diff::parse(
            &text,
            PathBuf::from("f"),
            false,
            false,
        ));
        app.set_diff_viewport_height(10);
        assert_eq!(app.diff().unwrap().lines.len(), 62);

        app.handle_event(key(KeyCode::PageDown));
        app.handle_event(key(KeyCode::PageDown));
        assert_eq!(app.diff_scroll(), 20);

        app.handle_event(key(KeyCode::End));
        assert_eq!(app.diff_scroll(), 52);
        app.handle_event(key(KeyCode::PageDown));
        assert_eq!(app.diff_scroll(), 52, "clamped at the end");

        app.handle_event(AppEvent::Key(KeyEvent::new(
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
        )));
        assert_eq!(app.diff_scroll(), 42);

        app.handle_event(key(KeyCode::Home));
        assert_eq!(app.diff_scroll(), 0);
        app.handle_event(AppEvent::Key(KeyEvent::new(
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
        )));
        assert_eq!(app.diff_scroll(), 0, "clamped at the top");
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

        assert_eq!(
            selected_path(&app),
            Some(PathBuf::from("b/d.txt")),
            "selection preserved"
        );
        let paths = row_paths(&app);
        assert!(
            paths.contains(&"b/e.txt".to_string()),
            "new file picked up: {paths:?}"
        );
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

    #[test]
    fn fs_changed_marks_relative_paths_recent_and_refreshes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        git(&root, &["init", "-q"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&root, &["add", "a.txt"]);
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
        let mut app = app_for(&root);

        // While agtx runs the agent edits the file.
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        let outside_dir = tempfile::tempdir().unwrap();
        let outside = outside_dir.path().join("x.txt");
        std::fs::write(&outside, "x\n").unwrap();

        app.handle_event(AppEvent::FsChanged(vec![
            root.join("a.txt"),
            // A watcher backend may report the canonical temp-dir path.
            root.canonicalize().unwrap().join("a.txt"),
            outside.clone(),
        ]));

        assert!(
            app.is_recent(Path::new("a.txt")),
            "repo-relative path marked recent"
        );
        assert!(
            !app.is_recent(&outside),
            "paths outside the root are dropped"
        );
        assert_eq!(app.status_message(), "updated");
        let row = app
            .rows()
            .iter()
            .find(|row| row.path == Path::new("a.txt"))
            .expect("a.txt is visible");
        assert!(row.status.is_some(), "refresh picked up the modification");
    }

    #[test]
    fn prune_recent_removes_entries_older_than_ten_seconds() {
        let mut app = test_app();
        let now = Instant::now();
        app.mark_recent(&[PathBuf::from("src/lib.rs")], now);

        app.dirty = false;
        app.prune_recent(now + Duration::from_secs(5));
        assert!(app.is_recent(Path::new("src/lib.rs")));
        assert!(!app.dirty, "pruning nothing must not set dirty");

        app.prune_recent(now + Duration::from_secs(11));
        assert!(!app.is_recent(Path::new("src/lib.rs")));
        assert!(app.dirty, "removing an entry sets dirty");
    }

    #[test]
    fn edit_target_returns_profile_and_absolute_path_for_files() {
        let mut app = test_app();
        app.handle_event(key(KeyCode::Char('G')));
        assert_eq!(selected_path(&app), Some(PathBuf::from("a.txt")));

        let (profile, path) = app.edit_target().expect("a file is selected");
        assert_eq!(profile.command, vec!["nvim", "+{line}", "{file}"]);
        assert_eq!(path, PathBuf::from("/repo/a.txt"));
        assert!(
            app.take_editor_request().is_none(),
            "only the `e` key queues a launch"
        );

        app.handle_event(key(KeyCode::Char('e')));
        let (profile, path) = app.take_editor_request().expect("`e` queues a launch");
        assert_eq!(profile.command, vec!["nvim", "+{line}", "{file}"]);
        assert_eq!(path, PathBuf::from("/repo/a.txt"));
        assert!(app.take_editor_request().is_none());
    }

    #[test]
    fn edit_target_is_none_on_a_directory_and_explains_in_status() {
        let mut app = test_app();
        assert_eq!(selected_path(&app), Some(PathBuf::from("src")));
        assert!(app.edit_target().is_none());

        app.handle_event(key(KeyCode::Char('e')));
        assert!(app.take_editor_request().is_none());
        assert!(app.status_message().contains("select a file"));
    }

    #[test]
    fn shift_enter_previews_an_unchanged_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("src/util")).unwrap();
        std::fs::write(root.join("src/util/mod.rs"), "pub fn x() {}\n").unwrap();
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["add", "."]);
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
        let mut app = app_for(&root);
        // rows: src, src/util, src/util/mod.rs, a.txt
        app.handle_event(key(KeyCode::Char('g')));
        app.handle_event(key(KeyCode::Char('j')));
        app.handle_event(key(KeyCode::Char('j')));
        assert_eq!(selected_path(&app), Some(PathBuf::from("src/util/mod.rs")));

        app.handle_event(AppEvent::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::SHIFT,
        )));

        assert_eq!(app.focus(), Focus::Diff);
        let view = app.diff().expect("preview loaded");
        assert_eq!(view.kind, ViewKind::File);
        assert_eq!(view.path, PathBuf::from("src/util/mod.rs"));
        assert!(view.lines.iter().any(|line| line.text.contains("pub fn x")));
        assert_eq!(app.status_message(), "file preview");
    }

    #[test]
    fn p_previews_and_s_switches_back_to_diff() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["add", "a.txt"]);
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
        let mut app = app_for(&root);
        app.handle_event(key(KeyCode::Char('p')));
        assert_eq!(app.diff().unwrap().kind, ViewKind::File);

        app.handle_event(key(KeyCode::Char('s')));
        assert_eq!(app.diff().unwrap().kind, ViewKind::Diff);
    }

    #[test]
    fn minus_collapses_to_current_directory_and_equals_expands() {
        let paths = vec![
            PathBuf::from("src/util/mod.rs"),
            PathBuf::from("docs/guide.md"),
            PathBuf::from("a.txt"),
        ];
        let git = GitState {
            branch: Some("main".to_string()),
            files: BTreeMap::new(),
            additions: 0,
            deletions: 0,
        };
        let mut app = App::new(PathBuf::from("/repo"), Config::default(), git, paths);
        // rows: docs, docs/guide.md, src, src/util, src/util/mod.rs, a.txt
        app.handle_event(key(KeyCode::Char('g')));
        for _ in 0..4 {
            app.handle_event(key(KeyCode::Char('j')));
        }
        assert_eq!(selected_path(&app), Some(PathBuf::from("src/util/mod.rs")));

        app.handle_event(key(KeyCode::Char('-')));
        assert_eq!(
            row_paths(&app),
            vec!["docs", "src", "src/util", "src/util/mod.rs", "a.txt"]
        );
        assert_eq!(selected_path(&app), Some(PathBuf::from("src/util/mod.rs")));

        app.handle_event(key(KeyCode::Char('=')));
        assert_eq!(
            row_paths(&app),
            vec![
                "docs",
                "docs/guide.md",
                "src",
                "src/util",
                "src/util/mod.rs",
                "a.txt",
            ]
        );
    }
}
