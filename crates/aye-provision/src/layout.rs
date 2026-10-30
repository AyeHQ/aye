//! Where an interpreter's installed packages live, as its files tell
//! ([`Layout`]).

use std::fs;
use std::path::{Path, PathBuf};

use aye_static::{EnvVars, ProcessEnv};

/// Where an interpreter's installed packages live.
pub(crate) struct Layout {
    /// The `pyvenv.cfg` of its venv (and of the base interpreter's, if
    /// any), with its content.
    pub(crate) configs: Vec<(PathBuf, Vec<u8>)>,
    /// Its `site-packages` directories, without repeats; the user's too
    /// when it isn't a venv's. Only when it has some of its own.
    pub(crate) sites: Vec<PathBuf>,
    /// The venv's own `site-packages`, when the interpreter is in a venv
    /// that leaves out the base interpreter's packages
    /// (`include-system-site-packages = false`, venv's default).
    isolated: Option<Vec<PathBuf>>,
}

impl Layout {
    /// `python` as given, and `real`, the file it resolves to; the user's
    /// home, for their own `site-packages`, is `env`'s.
    pub(crate) fn of(python: &Path, real: &Path, env: &ProcessEnv) -> Self {
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
            sites.extend(user_site_packages(env));
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
    pub(crate) fn importable(&self) -> &[PathBuf] {
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
/// import from, under `env`'s home directory.
fn user_site_packages(env: &ProcessEnv) -> Vec<PathBuf> {
    let Some(home) = env
        .get_os(EnvVars::HOME)
        .or_else(|| env.get_os(EnvVars::USERPROFILE))
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
