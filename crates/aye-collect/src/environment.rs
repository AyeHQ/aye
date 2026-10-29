//! What refinement depends on outside the project's files, for the
//! refinement cache's key: the interpreter and its installed packages, the
//! environment variables workers inherit, and the framework's arguments.
//! Also the names of the modules installed outside the standard library,
//! whose content the key doesn't hold, so that a file naming one in a
//! string isn't cached (`standalone.rs`), and whether the interpreter
//! installs a given module at all ([`Environment::installs`]: the binary
//! asks about pytest to choose the default adapters).

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use aye_events::Fnv128;
use aye_static::EnvVars;

/// What refinement depends on outside the project's files. Every part goes
/// into the refinement cache's key; see [`crate::Collector::with_environment`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Environment {
    /// The interpreter that refines files, absolute or relative to the
    /// project's cwd. It stands for itself (its path, the file it resolves
    /// to, that file's size and time) and for its installed packages (the
    /// entries of its `site-packages` directories, and the content of their
    /// `.pth` files). `None`, the default, or an interpreter whose
    /// `site-packages` can't be found, turns the cache off: entries made by
    /// another Python or other packages would be misread.
    pub interpreter: Option<PathBuf>,
    /// The arguments passed through to the framework (`aye -- …`). The
    /// project's `framework_args` are in the key too, so this may repeat
    /// them or stay empty.
    pub framework_args: Vec<String>,
    /// The environment variables workers see. `None`, the default, means
    /// aye's own, which workers inherit.
    pub variables: Option<Vec<(OsString, OsString)>>,
}

/// Variables shells and terminals change between runs without changing
/// anything a test could depend on; left out of the key so that a new
/// terminal or a `cd` doesn't refine everything again.
const SHELL_VARIABLES: &[&str] = &[
    "_",
    "OLDPWD",
    "PWD",
    "SHLVL",
    "TERM_SESSION_ID",
    "ITERM_SESSION_ID",
    "WINDOWID",
    "TMUX_PANE",
    "COLUMNS",
    "LINES",
];

impl Environment {
    /// A digest of everything this environment stands for, or `None` when
    /// the interpreter can't be identified.
    pub(crate) fn digest(&self, cwd: &Path) -> Option<u128> {
        let mut hash = Fnv128::new();
        interpreter(
            &aye_fs::absolute(cwd, self.interpreter.as_ref()?),
            &mut hash,
        )?;
        #[expect(
            clippy::disallowed_methods,
            reason = "the refinement cache's key holds the whole environment workers inherit"
        )]
        let mut variables = self
            .variables
            .clone()
            .unwrap_or_else(|| std::env::vars_os().collect());
        variables.retain(|(name, _)| {
            name.to_str()
                .is_none_or(|name| !SHELL_VARIABLES.contains(&name))
        });
        variables.sort();
        hash.part(b"variables");
        for (name, value) in &variables {
            hash.part(name.as_encoded_bytes());
            hash.part(value.as_encoded_bytes());
        }
        hash.part(b"framework args");
        for arg in &self.framework_args {
            hash.part(arg.as_bytes());
        }
        Some(hash.finish())
    }
}

/// Hash what identifies the interpreter at `python` and its installed
/// packages. `None` when it doesn't exist or its packages can't be found.
fn interpreter(python: &Path, hash: &mut Fnv128) -> Option<()> {
    let real = fs::canonicalize(python).ok()?;
    let meta = fs::metadata(&real).ok()?;
    hash.part(b"interpreter");
    hash.part(python.as_os_str().as_encoded_bytes());
    hash.part(real.as_os_str().as_encoded_bytes());
    hash.part(&meta.len().to_le_bytes());
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    hash.part(&modified.to_le_bytes());

    let layout = Layout::of(python, &real);
    for (config, text) in &layout.configs {
        hash.part(config.as_os_str().as_encoded_bytes());
        hash.part(text);
    }
    // Without packages of its own, the interpreter is not one aye can tell
    // apart from another (a shim, a wrapper script).
    if layout.sites.is_empty() {
        return None;
    }
    for site in layout.sites {
        hash.part(site.as_os_str().as_encoded_bytes());
        let Ok(entries) = fs::read_dir(&site) else {
            continue;
        };
        let mut names: Vec<OsString> = entries.filter_map(|e| Some(e.ok()?.file_name())).collect();
        names.sort();
        for name in names {
            hash.part(name.as_encoded_bytes());
            if Path::new(&name).extension().is_some_and(|ext| ext == "pth") {
                hash.part(&fs::read(site.join(&name)).unwrap_or_default());
            }
        }
    }
    Some(())
}

/// Where an interpreter's installed packages live.
struct Layout {
    /// The `pyvenv.cfg` of its venv (and of the base interpreter's, if
    /// any), with its content.
    configs: Vec<(PathBuf, Vec<u8>)>,
    /// Its `site-packages` directories, without repeats; the user's too
    /// when it isn't a venv's. Only when it has some of its own.
    sites: Vec<PathBuf>,
    /// The venv's own `site-packages`, when the interpreter is in a venv
    /// that leaves out the base interpreter's packages
    /// (`include-system-site-packages = false`, venv's default).
    isolated: Option<Vec<PathBuf>>,
}

impl Layout {
    /// `python` as given, and `real`, the file it resolves to.
    fn of(python: &Path, real: &Path) -> Self {
        // A venv's prefix, beside its `bin` (`Scripts` on Windows), then the
        // base interpreter's; on Windows an installation's own directory,
        // which holds `python.exe` beside `Lib`, is one too.
        let mut prefixes: Vec<&Path> = Vec::new();
        for exe in [python, real] {
            let parent = exe.parent();
            for prefix in [parent.and_then(Path::parent), parent]
                .into_iter()
                .flatten()
            {
                if !prefixes.contains(&prefix) {
                    prefixes.push(prefix);
                }
            }
        }
        let mut configs = Vec::new();
        let mut sites = Vec::new();
        let mut isolated = None;
        for prefix in prefixes {
            let found = site_packages(prefix);
            let config = prefix.join("pyvenv.cfg");
            if let Ok(text) = fs::read(&config) {
                if isolated.is_none()
                    && python.starts_with(prefix)
                    && !includes_system_site_packages(&text)
                {
                    isolated = Some(found.clone());
                }
                configs.push((config, text));
            }
            sites.extend(found);
        }
        if sites.is_empty() {
            return Self {
                configs,
                sites,
                isolated,
            };
        }
        if configs.is_empty() {
            sites.extend(user_site_packages());
        }
        let mut seen = Vec::new();
        sites.retain(|site| {
            let real = fs::canonicalize(site).unwrap_or_else(|_| site.clone());
            let new = !seen.contains(&real);
            seen.push(real);
            new
        });
        Self {
            configs,
            sites,
            isolated,
        }
    }

    /// The `site-packages` directories the interpreter imports from: its
    /// venv's own when the venv leaves out the base interpreter's packages,
    /// else all of [`Self::sites`].
    fn importable(&self) -> &[PathBuf] {
        self.isolated.as_deref().unwrap_or(&self.sites)
    }
}

/// Whether a `pyvenv.cfg` lets the venv import the base interpreter's
/// packages: its `include-system-site-packages`, `false` by default.
fn includes_system_site_packages(config: &[u8]) -> bool {
    String::from_utf8_lossy(config).lines().any(|line| {
        line.split_once('=').is_some_and(|(key, value)| {
            key.trim()
                .eq_ignore_ascii_case("include-system-site-packages")
                && value.trim().eq_ignore_ascii_case("true")
        })
    })
}

impl Environment {
    /// Whether the interpreter installs a top-level module named one of
    /// `names` (`pytest`), as far as its directories tell (as for the cache):
    /// `None` when its `site-packages` can't be found (no interpreter, a
    /// shim or wrapper script), so that a caller can tell "not installed"
    /// from "can't tell". A venv whose `pyvenv.cfg` leaves out the system's
    /// packages (`include-system-site-packages = false`, venv's default)
    /// counts only its own.
    pub fn installs(&self, cwd: &Path, names: &[&str]) -> Option<bool> {
        let python = aye_fs::absolute(cwd, self.interpreter.as_ref()?);
        let real = fs::canonicalize(&python).ok()?;
        let layout = Layout::of(&python, &real);
        if layout.importable().is_empty() {
            return None;
        }
        let installed = self.modules_in_sites(cwd, layout.importable());
        Some(names.iter().any(|name| installed.contains(*name)))
    }

    /// The top-level modules the interpreter may import from outside the
    /// standard library, as far as its directories tell: each entry of its
    /// `site-packages` directories, of the directories their `.pth` files
    /// and `PYTHONPATH` add, and the distributions installed there (their
    /// names and `top_level.txt`), which covers editable installs whose
    /// import hook maps a name elsewhere. Their content is not in the
    /// refinement cache's key, so a file naming one by string isn't cached
    /// (`standalone.rs`). Empty without an interpreter.
    pub(crate) fn installed_modules(&self, cwd: &Path) -> HashSet<String> {
        let Some(python) = &self.interpreter else {
            return HashSet::new();
        };
        let python = aye_fs::absolute(cwd, python);
        let Ok(real) = fs::canonicalize(&python) else {
            return HashSet::new();
        };
        self.modules_in_sites(cwd, &Layout::of(&python, &real).sites)
    }

    /// [`Self::installed_modules`] from the `site-packages` directories
    /// `sites`.
    fn modules_in_sites(&self, cwd: &Path, sites: &[PathBuf]) -> HashSet<String> {
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
        let pythonpath = match &self.variables {
            Some(variables) => variables
                .iter()
                .find(|(name, _)| name == EnvVars::PYTHONPATH)
                .map(|(_, value)| value.clone()),
            #[expect(
                clippy::disallowed_methods,
                reason = "the refinement cache's key, with the environment workers inherit"
            )]
            None => std::env::var_os(EnvVars::PYTHONPATH),
        };
        for dir in pythonpath.iter().flat_map(std::env::split_paths) {
            modules_in(&aye_fs::absolute(cwd, &dir), &mut names);
        }
        names
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

/// The package directories under an installation prefix:
/// `lib/pythonX.Y/site-packages` (POSIX), `Lib/site-packages` (Windows),
/// `lib/site-packages`, and Debian's `dist-packages`, also under
/// `<prefix>/local`.
fn site_packages(prefix: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for base in [prefix.to_path_buf(), prefix.join("local")] {
        for lib in ["lib", "lib64", "Lib"] {
            let lib = base.join(lib);
            let Ok(entries) = fs::read_dir(&lib) else {
                continue;
            };
            let mut dirs: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().starts_with("python"))
                .map(|e| e.path())
                .chain([lib.clone()])
                .flat_map(|dir| [dir.join("site-packages"), dir.join("dist-packages")])
                .filter(|site| site.is_dir())
                .collect();
            dirs.sort();
            out.extend(dirs);
        }
    }
    out
}

/// The user's own `site-packages`, which interpreters outside a venv also
/// import from.
#[expect(
    clippy::disallowed_methods,
    reason = "the refinement cache's key, with the environment workers inherit"
)]
fn user_site_packages() -> Vec<PathBuf> {
    let Some(home) =
        std::env::var_os(EnvVars::HOME).or_else(|| std::env::var_os(EnvVars::USERPROFILE))
    else {
        return Vec::new();
    };
    let home = PathBuf::from(home);
    let mut out = site_packages(&home.join(".local"));
    if let Ok(entries) = fs::read_dir(home.join("Library/Python")) {
        let mut versions: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path().join("lib/python/site-packages"))
            .filter(|site| site.is_dir())
            .collect();
        versions.sort();
        out.extend(versions);
    }
    out
}
