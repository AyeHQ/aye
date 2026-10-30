//! pytest's ignore rules: `collect_ignore`, `collect_ignore_glob`,
//! `--ignore` and `--ignore-glob`.

use std::path::Path;

use aye_pytest::conftest::in_confcutdir;
use aye_python_stdlib::fnmatch;
use aye_python_stdlib::os_path::normalize_absolute;

use crate::conftest::{self, Summary};
use crate::{PytestRecognizer, SETS};

/// Whether ignoring a path is certain, possible or ruled out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ignored {
    No,
    Maybe,
    Yes,
}

impl PytestRecognizer {
    /// Whether pytest ignores `path` (`collect_ignore`, `--ignore` and
    /// their glob forms).
    pub(crate) fn ignored(&self, path: &Path) -> Ignored {
        let path = normalize_absolute(path);
        if self
            .config
            .ignore
            .iter()
            .any(|p| normalize_absolute(p) == path)
        {
            return Ignored::Yes;
        }
        let text = path.to_string_lossy();
        if self
            .config
            .ignore_glob
            .iter()
            .any(|glob| fnmatch::fnmatch(&text, glob, SETS))
        {
            return Ignored::Yes;
        }
        let Some(parent) = path.parent() else {
            return Ignored::No;
        };
        let nearest = |pick: fn(&Summary) -> Option<&conftest::IgnoreList>| {
            parent
                .ancestors()
                .filter(|dir| in_confcutdir(&self.config.confcutdir, dir))
                .filter_map(|dir| self.conftest_in(dir))
                .find(|summary| pick(summary).is_some())
        };
        let mut verdict = Ignored::No;
        if let Some(summary) = nearest(|s| s.collect_ignore.as_ref()) {
            let list = summary.collect_ignore.as_ref().expect("picked");
            for (entry, conditional) in &list.entries {
                if normalize_absolute(&summary.dir.join(entry)) == path {
                    if !conditional {
                        return Ignored::Yes;
                    }
                    verdict = Ignored::Maybe;
                }
            }
            if list.unknown {
                verdict = Ignored::Maybe;
            }
        }
        if let Some(summary) = nearest(|s| s.collect_ignore_glob.as_ref()) {
            let list = summary.collect_ignore_glob.as_ref().expect("picked");
            for (entry, conditional) in &list.entries {
                let glob = normalize_absolute(&summary.dir.join(entry));
                if fnmatch::fnmatch(&text, &glob.to_string_lossy(), SETS) {
                    if !conditional {
                        return Ignored::Yes;
                    }
                    verdict = Ignored::Maybe;
                }
            }
            if list.unknown {
                verdict = Ignored::Maybe;
            }
        }
        verdict
    }
}
