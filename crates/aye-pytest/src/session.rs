//! pytest's session (`_pytest/main.py`): the directories it doesn't
//! collect from, and the tests `--deselect` drops, in aye's ids.

use std::path::Path;

/// pytest's `_in_venv`.
pub fn is_virtualenv(dir: &Path) -> bool {
    dir.join("pyvenv.cfg").is_file() || dir.join("conda-meta").join("history").is_file()
}

/// `--deselect`'s prefixes (`deselect`, pytest's node ids, relative to the
/// `rootdir`) as prefixes of aye's ids, which are relative to the project
/// `root`; none when the rootdir is outside the root.
pub fn deselect_ids(rootdir: &Path, root: &Path, deselect: &[String]) -> Vec<String> {
    let Ok(rel) = rootdir.strip_prefix(root) else {
        return Vec::new();
    };
    let rel = rel.to_string_lossy().replace('\\', "/");
    deselect
        .iter()
        .map(|prefix| {
            if rel.is_empty() {
                prefix.clone()
            } else {
                format!("{rel}/{prefix}")
            }
        })
        .collect()
}
