//! The modules an interpreter installs outside the standard library
//! ([`InstalledModules`]).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use aye_static::{EnvVars, ProcessEnv};

/// The top-level modules an interpreter may import from outside the
/// standard library, as far as its directories tell: each entry of its
/// `site-packages` directories, of the directories their `.pth` files and
/// `PYTHONPATH` add, and the distributions installed there (their names
/// and `top_level.txt`), which covers editable installs whose import hook
/// maps a name elsewhere.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstalledModules(HashSet<String>);

impl InstalledModules {
    /// Those of the `site-packages` directories `sites`, and of `env`'s
    /// `PYTHONPATH`, whose relative entries are `cwd`'s.
    pub(crate) fn in_sites(sites: &[PathBuf], cwd: &Path, env: &ProcessEnv) -> Self {
        let mut names = HashSet::new();
        for site in sites {
            let Ok(entries) = fs::read_dir(site) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                module_name(name, &mut names);
                if let Some(dist) = name
                    .strip_suffix(".dist-info")
                    .or_else(|| name.strip_suffix(".egg-info"))
                    .or_else(|| name.strip_suffix(".egg"))
                {
                    let dist = dist.split('-').next().unwrap_or(dist);
                    names.insert(dist.to_string());
                    names.insert(dist.to_lowercase());
                    if let Ok(text) = fs::read_to_string(entry.path().join("top_level.txt")) {
                        for line in text.lines() {
                            module_name(line.trim(), &mut names);
                        }
                    }
                } else if Path::new(name).extension().is_some_and(|ext| ext == "pth") {
                    let text = fs::read_to_string(entry.path()).unwrap_or_default();
                    for line in text.lines().map(str::trim_end) {
                        let executed = line.starts_with("import ") || line.starts_with("import\t");
                        if !line.is_empty() && !line.starts_with('#') && !executed {
                            modules_in(&site.join(line), &mut names);
                        }
                    }
                }
            }
        }
        for dir in env
            .get_os(EnvVars::PYTHONPATH)
            .iter()
            .flat_map(std::env::split_paths)
        {
            modules_in(&aye_fs::absolute(cwd, &dir), &mut names);
        }
        Self(names)
    }

    /// Whether a top-level module named `name` is among them.
    pub fn contains(&self, name: &str) -> bool {
        self.0.contains(name)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Modules by name, as a test or another reader lists them.
impl FromIterator<String> for InstalledModules {
    fn from_iter<I: IntoIterator<Item = String>>(names: I) -> Self {
        Self(names.into_iter().collect())
    }
}

/// The top-level modules in a directory on the import path.
fn modules_in(dir: &Path, names: &mut HashSet<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        if let Some(name) = entry.file_name().to_str() {
            module_name(name, names);
        }
    }
}

/// The module an entry of a directory on the import path provides: a
/// package or namespace directory (`name`), a source file (`name.py`) or an
/// extension module (`name.cpython-313-darwin.so`, `name.pyd`).
fn module_name(entry: &str, names: &mut HashSet<String>) {
    let (stem, rest) = entry.split_once('.').unwrap_or((entry, ""));
    let module = rest.is_empty()
        || ["py", "pyc", "pyw", "so", "pyd"]
            .iter()
            .any(|ext| rest == *ext || rest.ends_with(&format!(".{ext}")));
    let identifier = stem
        .chars()
        .next()
        .is_some_and(|c| c == '_' || c.is_alphabetic())
        && stem.chars().all(|c| c == '_' || c.is_alphanumeric());
    if module && identifier {
        names.insert(stem.to_string());
    }
}
