//! What collection walks: the explicit paths, normalized as pytest
//! normalizes its arguments, or else every recognizer's roots.

use std::path::{Path, PathBuf};

use aye_events::{Diagnostic, DiagnosticCode, DiagnosticPath, Severity};

use crate::Collector;
use crate::walk::{Mask, Tree};

impl Collector<'_> {
    /// Walk the targets, or the recognizers' roots; `false` when `tick`
    /// stopped a walk.
    pub(crate) fn discover(
        &self,
        tree: &mut Tree<'_>,
        targets: &[PathBuf],
        tick: &mut dyn FnMut(&Tree<'_>) -> bool,
    ) -> bool {
        if targets.is_empty() {
            for (root, mask) in self.roots() {
                if root.is_dir() {
                    if !tree.walk(&root, mask, tick) {
                        return false;
                    }
                } else if root.is_file() {
                    tree.visit(root, mask, true);
                }
            }
            return true;
        }
        let all = (0..self.recognizers.len()).fold(0, |all: Mask, i| all | 1 << i);
        for (abs, target) in self.normalize_targets(targets) {
            let rel = aye_fs::relative_through_root(&abs, self.project.root);
            let (code, message) = if abs.is_dir() {
                if !tree.walk(&abs, all, tick) {
                    return false;
                }
                continue;
            } else if abs.is_file() {
                let at = tree.visit(abs, all, true);
                let entry = &tree.entries[at];
                if entry.tests | entry.support != 0 {
                    continue;
                }
                (
                    DiagnosticCode::Unclaimed,
                    "no adapter collects this file".to_string(),
                )
            } else {
                let message = format!("file or directory not found: {}", target.display());
                (DiagnosticCode::NotFound, message)
            };
            tree.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code,
                path: DiagnosticPath::as_written(rel),
                span: None,
                message,
            });
        }
        true
    }

    /// The explicit paths, absolute and normalized, as pytest's
    /// `normalize_collection_arguments` leaves them: an existing path below
    /// another explicit directory, or repeating an earlier path, is dropped;
    /// the rest keep their order.
    fn normalize_targets<'t>(&self, targets: &'t [PathBuf]) -> Vec<(PathBuf, &'t PathBuf)> {
        let root = self.project.root;
        let resolved: Vec<(PathBuf, &PathBuf)> = targets
            .iter()
            .map(|target| {
                let abs = aye_fs::absolute(self.project.cwd, target);
                let abs = if abs.starts_with(root) {
                    abs
                } else {
                    aye_fs::relative_to(root, &abs).map_or(abs, |rel| root.join(rel))
                };
                (abs, target)
            })
            .collect();
        // Sorted, a directory comes just before what it subsumes.
        let mut order: Vec<usize> = (0..resolved.len()).collect();
        order.sort_by(|&a, &b| resolved[a].0.cmp(&resolved[b].0));
        let mut keep = vec![true; resolved.len()];
        let mut last: Option<&Path> = None;
        for at in order {
            let abs = resolved[at].0.as_path();
            // A missing path is reported whatever subsumes it, as in pytest.
            if !abs.exists() {
                continue;
            }
            if last.is_some_and(|by| abs == by || (abs.starts_with(by) && by.is_dir())) {
                keep[at] = false;
            } else {
                last = Some(abs);
            }
        }
        resolved
            .into_iter()
            .zip(keep)
            .filter_map(|(target, keep)| keep.then_some(target))
            .collect()
    }

    /// The recognizers' roots, in order and without repeats, each with the
    /// recognizers that listed it.
    fn roots(&self) -> Vec<(PathBuf, Mask)> {
        let mut roots: Vec<(PathBuf, Mask)> = Vec::new();
        for (i, (_, recognizer)) in self.recognizers.iter().enumerate() {
            for root in recognizer.roots(&self.project) {
                let root = aye_fs::absolute(self.project.cwd, &root);
                match roots.iter_mut().find(|(seen, _)| *seen == root) {
                    Some((_, mask)) => *mask |= 1 << i,
                    None => roots.push((root, 1 << i)),
                }
            }
        }
        roots
    }
}
