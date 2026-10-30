//! The walk's order: a directory's entries by name, a package's
//! `__init__.py` first ([`walk_order`]), and the same order for the paths
//! cases name ([`walk_cmp`], [`WalkKey`]).

use std::cmp::Ordering;

use aye_events::{ProjectPath, ProjectPathRef};

/// The order of a directory's entries in the walk: by name, as pytest's
/// `scandir` sorts them, except that `__init__.py` comes first, as pytest's
/// `Package` collects it (and unittest's discovery loads a package before
/// its contents). Names are the walk's own (`OsStr`) or a path's parts
/// (`str`), compared by their bytes alike.
pub(crate) fn walk_order<N: ?Sized + Ord + PartialEq<str>>(a: &N, b: &N) -> Ordering {
    let init = |name: &N| name != "__init__.py";
    init(a).cmp(&init(b)).then_with(|| a.cmp(b))
}

/// Two paths in the order the walk finds them: part by part, the first
/// parts that differ in [`walk_order`], and a directory before what it
/// holds.
pub(crate) fn walk_cmp(a: ProjectPathRef<'_>, b: ProjectPathRef<'_>) -> Ordering {
    let mut mine = a.as_str().split('/');
    let mut theirs = b.as_str().split('/');
    loop {
        match (mine.next(), theirs.next()) {
            (Some(a), Some(b)) if a == b => {}
            (Some(a), Some(b)) => return walk_order(a, b),
            (a, b) => return a.is_some().cmp(&b.is_some()),
        }
    }
}

/// A path, ordered by its place in the walk ([`walk_cmp`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalkKey(pub(crate) ProjectPath);

impl Ord for WalkKey {
    fn cmp(&self, other: &Self) -> Ordering {
        walk_cmp(self.0.as_path(), other.0.as_path())
    }
}

impl PartialOrd for WalkKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;
    use std::ffi::OsStr;

    use aye_events::ProjectPathRef;

    use super::{walk_cmp, walk_order};

    #[test]
    fn a_packages_init_comes_first_then_names_by_their_bytes() {
        let names = ["b.py", "__init__.py", "a.py", "B.py", "_a.py", "a"];
        let mut sorted = names;
        sorted.sort_by(|a, b| walk_order(*a, *b));
        assert_eq!(
            sorted,
            ["__init__.py", "B.py", "_a.py", "a", "a.py", "b.py"]
        );
        let mut walked = names.map(OsStr::new);
        walked.sort_by(|a, b| walk_order(*a, *b));
        assert_eq!(walked, sorted.map(OsStr::new));
    }

    #[test]
    fn paths_are_in_the_walks_order_part_by_part() {
        let cmp = |a, b| walk_cmp(ProjectPathRef::as_written(a), ProjectPathRef::as_written(b));
        assert_eq!(cmp("pkg/__init__.py", "pkg/a.py"), Ordering::Less);
        assert_eq!(cmp("pkg/z.py", "pkg/__init__.py"), Ordering::Greater);
        assert_eq!(cmp("pkg/sub/x.py", "pkg/t.py"), Ordering::Less);
        assert_eq!(cmp("pkg", "pkg/a.py"), Ordering::Less);
        assert_eq!(cmp("pkg/a.py", "pkg/a.py"), Ordering::Equal);
        assert_eq!(cmp("a.py", "a.py/x"), Ordering::Less);
    }
}
