//! aye's state directory, `<project root>/.aye`, and its buckets.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **The buckets keep aye 0.0.1's names** (`history.json`, `runs`,
//!   `cache`): renaming one would orphan the history, runs and caches users
//!   have.
//! - **The directory is marked as a cache** when aye creates it, whichever
//!   writer comes first (history, the run store or the refinement cache,
//!   each through [`prepare_dir`]): a `.gitignore` holding `*` and a
//!   `CACHEDIR.TAG` in the outermost directory created, unless that one is
//!   inside a marked directory already. A directory that already exists is
//!   left as it is, as pytest does, so pointing history at a directory of
//!   one's own never hides it from git. The one exception is the unmarked
//!   state directory aye 0.0.1 left when its refinement cache came first: an
//!   unmarked directory holding a `cache/refine` directory and nothing but
//!   aye's own files gets its markers.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Written into a directory aye creates.
const MARKERS: [(&str, &str); 2] = [
    (".gitignore", "# Created by aye automatically.\n*\n"),
    (
        "CACHEDIR.TAG",
        "Signature: 8a477f597d28d172789f06886806bc55\n\
         # This file is a cache directory tag created by aye.\n\
         # For information about cache directory tags, see:\n\
         #\thttps://bford.info/cachedir/spec.html\n",
    ),
];

/// What the temporary files history writes beside `history.json` start
/// with; they end in `.tmp`.
pub const HISTORY_TEMPORARY: &str = ".history.";

/// The refinement cache's directory in [`StateBucket::Cache`], holding one
/// directory per adapter.
pub const REFINE_CACHE: &str = "refine";

/// aye's state directory: what it keeps between runs, `.aye` under the
/// project root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDir {
    path: PathBuf,
}

impl StateDir {
    /// The directory's name under the project root.
    pub const NAME: &'static str = ".aye";

    /// The state directory of the project rooted at `root`.
    pub fn for_root(root: &Path) -> Self {
        Self {
            path: root.join(Self::NAME),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where `bucket` is kept.
    pub fn bucket(&self, bucket: StateBucket) -> PathBuf {
        self.path.join(bucket.name())
    }
}

/// What aye keeps in its state directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StateBucket {
    /// `history.json`: durations, outcomes and last failed (aye-history).
    History,
    /// `runs`: each run's log and summary, for `aye show` (aye-history's
    /// run store).
    Runs,
    /// `cache`: what only saves time; the refinement cache is
    /// `cache/refine/<adapter>` (aye-collect).
    Cache,
}

impl StateBucket {
    pub const ALL: [Self; 3] = [Self::History, Self::Runs, Self::Cache];

    /// The bucket's name in the state directory.
    pub const fn name(self) -> &'static str {
        match self {
            Self::History => "history.json",
            Self::Runs => "runs",
            Self::Cache => "cache",
        }
    }
}

/// Creates `dir` and whatever is missing above it, and marks the outermost
/// directory it creates as a cache: a `.gitignore` holding `*` and a
/// `CACHEDIR.TAG`, unless that directory is inside a marked one already. A
/// directory that exists is left alone (see the module docs for the one
/// exception). Whatever aye keeps in its state directory creates its
/// directory through this, so the markers are written whichever writer
/// creates the directory first.
pub fn prepare_dir(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        if is_unmarked_state(dir) {
            mark(dir)?;
        }
        return Ok(());
    }
    let outermost = dir
        .ancestors()
        .take_while(|ancestor| !ancestor.as_os_str().is_empty() && !ancestor.exists())
        .last()
        .unwrap_or(dir)
        .to_path_buf();
    fs::create_dir_all(dir)?;
    let inside_a_cache = outermost
        .parent()
        .is_some_and(|parent| parent.join(MARKERS[1].0).is_file());
    if inside_a_cache {
        return Ok(());
    }
    mark(&outermost)
}

/// Writes the markers into `dir`, keeping any that exist.
fn mark(dir: &Path) -> io::Result<()> {
    for (name, content) in MARKERS {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(name))
        {
            Ok(mut file) => file.write_all(content.as_bytes())?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// A state directory aye created without markers: no marker at all, a
/// `cache/refine` directory (the refinement cache's), and nothing but the
/// buckets and history's temporary files.
fn is_unmarked_state(dir: &Path) -> bool {
    if MARKERS.iter().any(|(name, _)| dir.join(name).exists())
        || !dir
            .join(StateBucket::Cache.name())
            .join(REFINE_CACHE)
            .is_dir()
    {
        return false;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.into_iter().all(|entry| {
        entry.is_ok_and(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            StateBucket::ALL.iter().any(|bucket| name == bucket.name())
                || (name.starts_with(HISTORY_TEMPORARY) && name.ends_with(".tmp"))
        })
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{StateBucket, StateDir, prepare_dir};

    fn assert_marked(dir: &Path) {
        let gitignore = fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert!(gitignore.lines().any(|line| line == "*"), "{gitignore}");
        let tag = fs::read_to_string(dir.join("CACHEDIR.TAG")).unwrap();
        assert!(tag.starts_with("Signature: 8a477f597d28d172789f06886806bc55\n"));
    }

    fn assert_unmarked(dir: &Path) {
        assert!(!dir.join(".gitignore").exists(), "{}", dir.display());
        assert!(!dir.join("CACHEDIR.TAG").exists(), "{}", dir.display());
    }

    #[test]
    fn the_buckets_keep_their_names() {
        let state = StateDir::for_root(Path::new("/p"));
        assert_eq!(state.path(), Path::new("/p/.aye"));
        assert_eq!(
            StateBucket::ALL.map(|bucket| state.bucket(bucket)),
            ["/p/.aye/history.json", "/p/.aye/runs", "/p/.aye/cache"].map(PathBuf::from)
        );
    }

    #[test]
    fn the_outermost_directory_created_is_marked_once() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("nested/.aye");
        prepare_dir(&state.join("cache")).unwrap();
        assert_marked(&dir.path().join("nested"));
        assert_unmarked(&state);
        // Inside a marked directory, nothing is marked again.
        prepare_dir(&dir.path().join("nested/runs")).unwrap();
        assert_unmarked(&dir.path().join("nested/runs"));
        // A directory that exists is left alone, its markers too.
        fs::write(dir.path().join("nested/.gitignore"), "mine\n").unwrap();
        prepare_dir(&dir.path().join("nested")).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("nested/.gitignore")).unwrap(),
            "mine\n"
        );
        prepare_dir(dir.path()).unwrap();
        assert_unmarked(dir.path());
    }

    #[test]
    fn an_unmarked_state_directory_from_0_0_1_is_marked() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join(".aye");
        fs::create_dir_all(state.join("cache/refine/pytest")).unwrap();
        fs::create_dir_all(state.join("runs")).unwrap();
        fs::write(state.join("history.json"), "{}").unwrap();
        fs::write(state.join(".history.x1.tmp"), "").unwrap();
        prepare_dir(&state).unwrap();
        assert_marked(&state);

        // With anything of the user's in it, it is left alone.
        let other = tempfile::tempdir().unwrap();
        let state = other.path().join(".aye");
        fs::create_dir_all(state.join("cache/refine/pytest")).unwrap();
        fs::write(state.join("notes.txt"), "mine").unwrap();
        prepare_dir(&state).unwrap();
        assert_unmarked(&state);
    }
}
