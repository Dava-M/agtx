use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::git::status::{FileStatus, StatusKind};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub changed: usize,
    pub added: usize,
    pub deleted: usize,
    pub untracked: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub node: usize,
    pub depth: usize,
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub status: Option<FileStatus>,
    pub counts: Counts,
    pub has_changed_descendant: bool,
    pub collapsed: bool,
}

struct Node {
    name: std::ffi::OsString,
    path: PathBuf,
    is_dir: bool,
    children: Vec<usize>,
    status: Option<FileStatus>,
    counts: Counts,
    has_changed_descendant: bool,
}

/// Repository file tree. Node 0 is the (hidden) root. Collapse state is
/// keyed by path so `update_statuses` rebuilds preserve it.
pub struct Tree {
    nodes: Vec<Node>,
    rows: Vec<Row>,
    collapsed: HashSet<PathBuf>,
    changed_only: bool,
}

pub fn build(paths: Vec<PathBuf>, statuses: &BTreeMap<PathBuf, FileStatus>) -> Tree {
    let mut tree = Tree {
        nodes: vec![Node {
            name: OsString::new(),
            path: PathBuf::new(),
            is_dir: true,
            children: Vec::new(),
            status: None,
            counts: Counts::default(),
            has_changed_descendant: false,
        }],
        rows: Vec::new(),
        collapsed: HashSet::new(),
        changed_only: false,
    };
    for path in paths {
        tree.insert(&path);
    }
    for (path, status) in statuses {
        tree.insert(path);
        if let Some(idx) = tree.find(path) {
            tree.nodes[idx].status = Some(status.clone());
        }
    }
    tree.finalize();
    tree
}

impl Tree {
    fn insert(&mut self, path: &Path) {
        let components: Vec<_> = path.components().collect();
        if components.is_empty() {
            return;
        }
        let mut current = 0usize;
        for (pos, comp) in components.iter().enumerate() {
            let is_last = pos + 1 == components.len();
            let existing = self.nodes[current]
                .children
                .iter()
                .copied()
                .find(|&i| self.nodes[i].name.as_os_str() == comp.as_os_str());
            current = match existing {
                Some(i) => i,
                None => {
                    let idx = self.nodes.len();
                    self.nodes.push(Node {
                        name: comp.as_os_str().to_os_string(),
                        path: self.nodes[current].path.join(comp.as_os_str()),
                        is_dir: !is_last,
                        children: Vec::new(),
                        status: None,
                        counts: Counts::default(),
                        has_changed_descendant: false,
                    });
                    self.nodes[current].children.push(idx);
                    idx
                }
            };
        }
    }

    fn find(&self, path: &Path) -> Option<usize> {
        let mut current = 0usize;
        let mut components = path.components();
        let first = components.next()?;
        let mut step = self.nodes[current]
            .children
            .iter()
            .copied()
            .find(|&i| self.nodes[i].name.as_os_str() == first.as_os_str());
        for comp in components {
            current = step?;
            step = self.nodes[current]
                .children
                .iter()
                .copied()
                .find(|&i| self.nodes[i].name.as_os_str() == comp.as_os_str());
        }
        step
    }

    fn finalize(&mut self) {
        self.sort_children();
        self.recompute();
        self.rebuild_rows();
    }

    fn sort_children(&mut self) {
        for i in 0..self.nodes.len() {
            let mut children: Vec<(usize, (bool, String, String))> =
                std::mem::take(&mut self.nodes[i].children)
                    .into_iter()
                    .map(|c| {
                        let name = self.nodes[c].name.to_string_lossy().into_owned();
                        (c, (!self.nodes[c].is_dir, name.to_lowercase(), name))
                    })
                    .collect();
            children.sort_by(|a, b| a.1.cmp(&b.1));
            self.nodes[i].children = children.into_iter().map(|(c, _)| c).collect();
        }
    }

    fn recompute(&mut self) {
        for i in (0..self.nodes.len()).rev() {
            if self.nodes[i].is_dir {
                let children = self.nodes[i].children.clone();
                let mut counts = Counts::default();
                let mut has = false;
                for child in children {
                    let cc = self.nodes[child].counts;
                    counts.changed += cc.changed;
                    counts.added += cc.added;
                    counts.deleted += cc.deleted;
                    counts.untracked += cc.untracked;
                    has = has || self.nodes[child].has_changed_descendant;
                }
                self.nodes[i].counts = counts;
                self.nodes[i].has_changed_descendant = has;
            } else {
                let (counts, has) = match &self.nodes[i].status {
                    Some(status) => (status_counts(status), true),
                    None => (Counts::default(), false),
                };
                self.nodes[i].counts = counts;
                self.nodes[i].has_changed_descendant = has;
            }
        }
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        let root_children = self.nodes[0].children.clone();
        for child in root_children {
            self.push_row(child, 1, &mut rows);
        }
        self.rows = rows;
    }

    fn push_row(&self, idx: usize, depth: usize, out: &mut Vec<Row>) {
        let node = &self.nodes[idx];
        if node.is_dir {
            if self.changed_only && !node.has_changed_descendant {
                return;
            }
            let collapsed = self.collapsed.contains(&node.path);
            out.push(Row {
                node: idx,
                depth,
                name: node.name.to_string_lossy().into_owned(),
                path: node.path.clone(),
                is_dir: true,
                status: None,
                counts: node.counts,
                has_changed_descendant: node.has_changed_descendant,
                collapsed,
            });
            if !collapsed {
                let children = node.children.clone();
                for child in children {
                    self.push_row(child, depth + 1, out);
                }
            }
        } else {
            if self.changed_only && node.status.is_none() {
                return;
            }
            out.push(Row {
                node: idx,
                depth,
                name: node.name.to_string_lossy().into_owned(),
                path: node.path.clone(),
                is_dir: false,
                status: node.status.clone(),
                counts: node.counts,
                has_changed_descendant: node.has_changed_descendant,
                collapsed: false,
            });
        }
    }

    pub fn set_changed_only(&mut self, value: bool) {
        self.changed_only = value;
        self.rebuild_rows();
    }

    #[allow(dead_code)] // consumed by an incremental refresh path (not yet wired)
    pub fn update_statuses(&mut self, statuses: &BTreeMap<PathBuf, FileStatus>) {
        for node in &mut self.nodes {
            if !node.is_dir {
                node.status = statuses.get(&node.path).cloned();
            }
        }
        self.recompute();
        self.rebuild_rows();
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn toggle_collapse(&mut self, row: usize) -> bool {
        let path = match self.rows.get(row) {
            Some(r) if r.is_dir => r.path.clone(),
            _ => return false,
        };
        if !self.collapsed.remove(&path) {
            self.collapsed.insert(path);
        }
        self.rebuild_rows();
        true
    }

    pub fn expand_all(&mut self) {
        self.collapsed.clear();
        self.rebuild_rows();
    }

    /// Collapse every directory except the selected path's ancestor chain.
    /// A directory anchor stays expanded; a file anchor keeps its parent
    /// directory and all higher ancestors expanded.
    pub fn collapse_to(&mut self, anchor: &Path) {
        self.collapsed.clear();
        let anchor_is_dir = self
            .find(anchor)
            .map(|idx| self.nodes[idx].is_dir)
            .unwrap_or(false);
        let mut keep: HashSet<PathBuf> = HashSet::new();
        if anchor_is_dir {
            keep.insert(anchor.to_path_buf());
        }
        let mut parent = anchor.parent();
        while let Some(path) = parent {
            if path.as_os_str().is_empty() {
                break;
            }
            keep.insert(path.to_path_buf());
            parent = path.parent();
        }
        for node in &self.nodes {
            if node.is_dir && !node.path.as_os_str().is_empty() && !keep.contains(&node.path) {
                self.collapsed.insert(node.path.clone());
            }
        }
        self.rebuild_rows();
    }

    #[allow(dead_code)] // no key bound to it in the MVP
    pub fn collapse_all(&mut self) {
        for node in &self.nodes {
            if node.is_dir && !node.path.as_os_str().is_empty() {
                self.collapsed.insert(node.path.clone());
            }
        }
        self.rebuild_rows();
    }

    pub fn find_row(&self, path: &Path) -> Option<usize> {
        self.rows.iter().position(|r| r.path.as_path() == path)
    }
}

fn status_counts(status: &FileStatus) -> Counts {
    Counts {
        changed: 1,
        added: matches!(status.kind, StatusKind::Added | StatusKind::Untracked) as usize,
        deleted: (status.kind == StatusKind::Deleted) as usize,
        untracked: (status.kind == StatusKind::Untracked) as usize,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::status::StatusKind;

    fn sample_paths() -> Vec<PathBuf> {
        vec![
            PathBuf::from("a.txt"),
            PathBuf::from("B.txt"),
            PathBuf::from("src/lib.rs"),
            PathBuf::from("src/util/mod.rs"),
            PathBuf::from("src/util/helper.rs"),
        ]
    }

    fn status(path: &str, kind: StatusKind, index: char, worktree: char) -> (PathBuf, FileStatus) {
        (
            PathBuf::from(path),
            FileStatus {
                path: PathBuf::from(path),
                index,
                worktree,
                kind,
                orig_path: None,
                staged: index != '.',
            },
        )
    }

    fn row_paths(tree: &Tree) -> Vec<String> {
        tree.rows()
            .iter()
            .map(|r| r.path.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn nested_build_dirs_first_case_insensitive() {
        let tree = build(sample_paths(), &BTreeMap::new());
        assert_eq!(
            row_paths(&tree),
            vec![
                "src",
                "src/util",
                "src/util/helper.rs",
                "src/util/mod.rs",
                "src/lib.rs",
                "a.txt",
                "B.txt",
            ]
        );
        assert_ne!(tree.rows()[0].node, 0, "rows reference non-root node ids");
        assert_eq!(tree.rows()[0].depth, 1);
        assert_eq!(tree.rows()[1].depth, 2);
        assert_eq!(tree.rows()[2].depth, 3);
        assert!(tree.rows()[0].is_dir);
        assert!(!tree.rows()[3].is_dir);
    }

    #[test]
    fn collapse_and_expand() {
        let mut tree = build(sample_paths(), &BTreeMap::new());
        let src_row = tree.find_row(Path::new("src")).unwrap();
        assert!(tree.toggle_collapse(src_row));
        let paths = row_paths(&tree);
        assert!(paths.contains(&"src".to_string()));
        assert!(!paths.contains(&"src/util".to_string()));
        assert!(!paths.contains(&"src/lib.rs".to_string()));
        assert!(tree.rows()[0].collapsed);

        assert!(tree.toggle_collapse(tree.find_row(Path::new("src")).unwrap()));
        assert!(tree.rows().iter().any(|r| r.path == Path::new("src/util")));

        let file_row = tree.find_row(Path::new("a.txt")).unwrap();
        assert!(!tree.toggle_collapse(file_row), "files cannot collapse");
    }

    #[test]
    fn counts_aggregate_by_kind() {
        let statuses: BTreeMap<PathBuf, FileStatus> = [
            status("a.txt", StatusKind::Modified, 'M', '.'),
            status("src/lib.rs", StatusKind::Deleted, 'D', '.'),
            status("src/util/mod.rs", StatusKind::Untracked, '?', '.'),
        ]
        .into_iter()
        .collect();
        let tree = build(sample_paths(), &statuses);

        let src = &tree.rows()[tree.find_row(Path::new("src")).unwrap()];
        assert_eq!(
            src.counts,
            Counts {
                changed: 2,
                added: 1,
                deleted: 1,
                untracked: 1
            }
        );
        assert!(src.has_changed_descendant);
        assert_eq!(src.status, None);

        let a = &tree.rows()[tree.find_row(Path::new("a.txt")).unwrap()];
        assert_eq!(
            a.counts,
            Counts {
                changed: 1,
                added: 0,
                deleted: 0,
                untracked: 0
            }
        );
        assert!(
            a.has_changed_descendant,
            "file with a status counts as changed"
        );
        assert_eq!(a.status.as_ref().unwrap().kind, StatusKind::Modified);

        let b = &tree.rows()[tree.find_row(Path::new("B.txt")).unwrap()];
        assert_eq!(b.counts.changed, 0);
        assert!(!b.has_changed_descendant);

        let lib = &tree.rows()[tree.find_row(Path::new("src/lib.rs")).unwrap()];
        assert_eq!(
            lib.counts,
            Counts {
                changed: 1,
                added: 0,
                deleted: 1,
                untracked: 0
            }
        );
    }

    #[test]
    fn changed_only_hides_unchanged_files_and_clean_dirs() {
        let statuses: BTreeMap<PathBuf, FileStatus> = [
            status("a.txt", StatusKind::Modified, 'M', '.'),
            status("src/lib.rs", StatusKind::Deleted, 'D', '.'),
            status("src/util/mod.rs", StatusKind::Untracked, '?', '.'),
        ]
        .into_iter()
        .collect();
        let mut tree = build(sample_paths(), &statuses);
        tree.set_changed_only(true);
        assert_eq!(
            row_paths(&tree),
            vec!["src", "src/util", "src/util/mod.rs", "src/lib.rs", "a.txt"]
        );

        tree.set_changed_only(false);
        assert_eq!(tree.rows().len(), 7);
    }

    #[test]
    fn find_row_locates_visible_paths() {
        let mut tree = build(sample_paths(), &BTreeMap::new());
        let idx = tree.find_row(Path::new("src/util/mod.rs")).unwrap();
        assert_eq!(tree.rows()[idx].path, PathBuf::from("src/util/mod.rs"));
        assert_eq!(tree.find_row(Path::new("missing.txt")), None);

        tree.collapse_all();
        assert_eq!(row_paths(&tree), vec!["src", "a.txt", "B.txt"]);
        assert_eq!(tree.find_row(Path::new("src/lib.rs")), None);

        tree.expand_all();
        assert_eq!(tree.rows().len(), 7);
    }

    #[test]
    fn collapse_to_keeps_selected_directory_chain_open() {
        let paths = vec![
            PathBuf::from("src/util/mod.rs"),
            PathBuf::from("docs/guide.md"),
            PathBuf::from("a.txt"),
        ];
        let mut tree = build(paths, &BTreeMap::new());

        tree.collapse_to(Path::new("src/util/mod.rs"));
        assert_eq!(
            row_paths(&tree),
            vec!["docs", "src", "src/util", "src/util/mod.rs", "a.txt"]
        );

        tree.collapse_to(Path::new("src"));
        assert_eq!(row_paths(&tree), vec!["docs", "src", "src/util", "a.txt"]);
    }

    #[test]
    fn update_statuses_recomputes_and_preserves_collapse() {
        let mut tree = build(sample_paths(), &BTreeMap::new());
        let src_row = tree.find_row(Path::new("src")).unwrap();
        assert!(tree.toggle_collapse(src_row));

        let statuses: BTreeMap<PathBuf, FileStatus> =
            [status("src/util/helper.rs", StatusKind::Added, 'A', '.')]
                .into_iter()
                .collect();
        tree.update_statuses(&statuses);

        let paths = row_paths(&tree);
        assert!(paths.contains(&"src".to_string()), "collapse preserved");
        assert!(!paths.contains(&"src/util".to_string()));

        let src = &tree.rows()[tree.find_row(Path::new("src")).unwrap()];
        assert_eq!(
            src.counts,
            Counts {
                changed: 1,
                added: 1,
                deleted: 0,
                untracked: 0
            }
        );
        assert!(src.has_changed_descendant);

        tree.expand_all();
        let helper = &tree.rows()[tree.find_row(Path::new("src/util/helper.rs")).unwrap()];
        assert_eq!(helper.status.as_ref().unwrap().kind, StatusKind::Added);
        assert_eq!(
            helper.counts,
            Counts {
                changed: 1,
                added: 1,
                deleted: 0,
                untracked: 0
            }
        );
        assert!(helper.has_changed_descendant);
    }
}
