//! The walk's order: a directory's entries by name, a package's
//! `__init__.py` first, and the same order for the paths cases name.

/// The order of a directory's entries in the walk: by name, as pytest's
/// `scandir` sorts them, except that `__init__.py` comes first, as pytest's
/// `Package` collects it (and unittest's discovery loads a package before
/// its contents).
pub(crate) fn walk_order(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> std::cmp::Ordering {
    let init = |name: &std::ffi::OsStr| name != "__init__.py";
    init(a).cmp(&init(b)).then_with(|| a.cmp(b))
}

/// A path's place in the walk ([`walk_order`] at each level), with
/// the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalkKey(pub(crate) String);

impl WalkKey {
    pub(crate) fn new(path: &str) -> Self {
        Self(path.to_string())
    }
}

impl Ord for WalkKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let mut mine = self.0.split('/');
        let mut theirs = other.0.split('/');
        loop {
            match (mine.next(), theirs.next()) {
                (Some(a), Some(b)) if a == b => {}
                (Some(a), Some(b)) => {
                    return walk_order(std::ffi::OsStr::new(a), std::ffi::OsStr::new(b));
                }
                (a, b) => return a.is_some().cmp(&b.is_some()),
            }
        }
    }
}

impl PartialOrd for WalkKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
