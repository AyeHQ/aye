//! What the refinement cache's key holds of the environment refinement
//! runs in ([`EnvironmentFingerprint`]).

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use aye_events::{ContentDigest, Fnv128};
use aye_static::ProcessEnv;

use crate::installed::InstalledModules;
use crate::layout::Layout;

/// The environment refinement runs in, as the refinement cache's key holds
/// it: the interpreter that refines files, the environment variables its
/// workers inherit, and the arguments passed through to the framework.
/// What these stand for is read from the interpreter's files when it is
/// asked for ([`Self::digest`], [`Self::installed_modules`]): a run that
/// refines nothing reads none of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvironmentFingerprint {
    /// The interpreter, absolute or relative to the project's cwd. It
    /// stands for itself (its path, the file it resolves to, that file's
    /// size and time) and for its installed packages (the entries of its
    /// `site-packages` directories, and the content of their `.pth` files).
    pub interpreter: PathBuf,
    /// The arguments passed through to the framework (`aye -- …`).
    pub framework_args: Vec<String>,
    /// The environment workers inherit: the binary's, as it read it. The
    /// user's home and `PYTHONPATH` are read from it too.
    pub variables: ProcessEnv,
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

impl EnvironmentFingerprint {
    /// A digest of everything the environment stands for, or `None` when
    /// the interpreter can't be identified: it doesn't exist, or has no
    /// `site-packages` of its own (a shim, a wrapper script). Entries made
    /// by another Python or other packages would be misread, so the cache
    /// is off then.
    pub fn digest(&self, cwd: &Path) -> Option<ContentDigest> {
        let mut hash = Fnv128::new();
        interpreter(
            &aye_fs::absolute(cwd, &self.interpreter),
            &self.variables,
            &mut hash,
        )?;
        let mut variables: Vec<(&OsStr, &OsStr)> = self
            .variables
            .iter()
            .filter(|(name, _)| {
                name.to_str()
                    .is_none_or(|name| !SHELL_VARIABLES.contains(&name))
            })
            .collect();
        variables.sort();
        hash.part(b"variables");
        for (name, value) in variables {
            hash.part(name.as_encoded_bytes());
            hash.part(value.as_encoded_bytes());
        }
        hash.part(b"framework args");
        for arg in &self.framework_args {
            hash.part(arg.as_bytes());
        }
        Some(hash.into())
    }

    /// The top-level modules the interpreter may import from outside the
    /// standard library ([`InstalledModules`]), whose content the key
    /// doesn't hold. Empty when the interpreter doesn't exist.
    pub fn installed_modules(&self, cwd: &Path) -> InstalledModules {
        let python = aye_fs::absolute(cwd, &self.interpreter);
        let Ok(real) = fs::canonicalize(&python) else {
            return InstalledModules::default();
        };
        let layout = Layout::of(&python, &real, &self.variables);
        InstalledModules::in_sites(&layout.sites, cwd, &self.variables)
    }

    /// Whether the interpreter installs a top-level module named one of
    /// `names` (`pytest`), as far as its directories tell (as for the key):
    /// `None` when its `site-packages` can't be found (a missing
    /// interpreter, a shim or wrapper script), so that a caller can tell
    /// "not installed" from "can't tell". A venv whose `pyvenv.cfg` leaves
    /// out the system's packages (`include-system-site-packages = false`,
    /// venv's default) counts only its own.
    pub fn installs(&self, cwd: &Path, names: &[&str]) -> Option<bool> {
        let python = aye_fs::absolute(cwd, &self.interpreter);
        let real = fs::canonicalize(&python).ok()?;
        let layout = Layout::of(&python, &real, &self.variables);
        if layout.importable().is_empty() {
            return None;
        }
        let installed = InstalledModules::in_sites(layout.importable(), cwd, &self.variables);
        Some(names.iter().any(|name| installed.contains(name)))
    }
}

/// Hash what identifies the interpreter at `python` and its installed
/// packages. `None` when it doesn't exist or its packages can't be found.
fn interpreter(python: &Path, env: &ProcessEnv, hash: &mut Fnv128) -> Option<()> {
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

    let layout = Layout::of(python, &real, env);
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
        let mut names: Vec<_> = entries.filter_map(|e| Some(e.ok()?.file_name())).collect();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: impl AsRef<[u8]>) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    /// A fake interpreter in a venv: `<dir>/venv/bin/python`, its
    /// `pyvenv.cfg` and an empty `site-packages`.
    fn interpreter(dir: &Path) -> PathBuf {
        write(dir, "venv/pyvenv.cfg", "home = /usr/bin\n");
        fs::create_dir_all(dir.join("venv/lib/python3.13/site-packages")).unwrap();
        write(dir, "venv/bin/python", "");
        dir.join("venv/bin/python")
    }

    /// The environment's digest, which the refinement cache's key holds,
    /// pinned to what it hashes, each an FNV-1a 128 part: the interpreter (its
    /// path, the file it resolves to, that file's size and time), its venv's
    /// `pyvenv.cfg` and its content, each `site-packages` directory with its
    /// entries in order (and a `.pth` file's content after its name), then the
    /// variables but the shell's, sorted, then the framework's arguments.
    /// Entries an earlier aye wrote keep answering only while it hashes the
    /// same. `#[cfg(unix)]`: there a temporary directory has one spelling once
    /// resolved, so the venv is found once.
    #[cfg(unix)]
    #[test]
    fn the_environments_digest_is_pinned() {
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        // As the interpreter resolves to itself: no second spelling of the venv.
        let root = fs::canonicalize(dir.path()).unwrap();
        let python = interpreter(&root);
        let site = root.join("venv/lib/python3.13/site-packages");
        write(&site, "b.pth", "/src\n");
        write(&site, "a/__init__.py", "");
        let modified = Duration::from_secs(1_700_000_000);
        fs::File::options()
            .write(true)
            .open(&python)
            .unwrap()
            .set_modified(UNIX_EPOCH + modified)
            .unwrap();
        let environment = EnvironmentFingerprint {
            interpreter: python.clone(),
            framework_args: vec!["-x".into(), "--lf".into()],
            variables: ProcessEnv::from_vars([
                ("B", "2"),
                ("SHLVL", "3"),
                ("A", "1"),
                ("PWD", "/x"),
            ]),
        };

        let mut hash = Fnv128::new();
        hash.part(b"interpreter");
        hash.part(python.as_os_str().as_encoded_bytes());
        hash.part(python.as_os_str().as_encoded_bytes());
        hash.part(&0u64.to_le_bytes());
        hash.part(&modified.as_nanos().to_le_bytes());
        hash.part(root.join("venv/pyvenv.cfg").as_os_str().as_encoded_bytes());
        hash.part(b"home = /usr/bin\n");
        hash.part(site.as_os_str().as_encoded_bytes());
        hash.part(b"a");
        hash.part(b"b.pth");
        hash.part(b"/src\n");
        hash.part(b"variables");
        for part in ["A", "1", "B", "2"] {
            hash.part(part.as_bytes());
        }
        hash.part(b"framework args");
        hash.part(b"-x");
        hash.part(b"--lf");
        let digest = ContentDigest::from(hash);
        assert_eq!(environment.digest(&root), Some(digest));
        // A relative interpreter is the cwd's.
        let relative = EnvironmentFingerprint {
            interpreter: PathBuf::from("venv/bin/python"),
            ..environment.clone()
        };
        assert_eq!(relative.digest(&root), Some(digest));

        // An interpreter outside a venv stands for the user's packages too,
        // under the home the environment gives.
        write(&root, "base/bin/python3", "");
        let base_site = root.join("base/lib/python3.13/site-packages");
        write(&base_site, "c.py", "");
        let user_site = root.join("home/.local/lib/python3.13/site-packages");
        write(&user_site, "d.py", "");
        let python = root.join("base/bin/python3");
        fs::File::options()
            .write(true)
            .open(&python)
            .unwrap()
            .set_modified(UNIX_EPOCH + modified)
            .unwrap();
        let home = root.join("home");
        let base = EnvironmentFingerprint {
            interpreter: python.clone(),
            framework_args: Vec::new(),
            variables: ProcessEnv::from_vars([("HOME", home.as_os_str())]),
        };
        let mut hash = Fnv128::new();
        hash.part(b"interpreter");
        hash.part(python.as_os_str().as_encoded_bytes());
        hash.part(python.as_os_str().as_encoded_bytes());
        hash.part(&0u64.to_le_bytes());
        hash.part(&modified.as_nanos().to_le_bytes());
        hash.part(base_site.as_os_str().as_encoded_bytes());
        hash.part(b"c.py");
        hash.part(user_site.as_os_str().as_encoded_bytes());
        hash.part(b"d.py");
        hash.part(b"variables");
        hash.part(b"HOME");
        hash.part(home.as_os_str().as_encoded_bytes());
        hash.part(b"framework args");
        assert_eq!(base.digest(&root), Some(ContentDigest::from(hash)));
    }

    #[test]
    fn installed_modules_are_listed_from_the_interpreters_paths() {
        let dir = tempfile::tempdir().unwrap();
        let python = interpreter(dir.path());
        let site = dir.path().join("venv/lib/python3.13/site-packages");
        for entry in [
            "pkg/__init__.py",
            "single.py",
            "ext.cpython-313-darwin.so",
            "winext.pyd",
            "Some_Dist-1.0.dist-info/top_level.txt",
            "old.egg-info/top_level.txt",
            "notes.txt",
        ] {
            write(&site, entry, "");
        }
        write(&site, "Some_Dist-1.0.dist-info/top_level.txt", "toplevel\n");
        write(&site, "old.egg-info/top_level.txt", "oldmod\n");
        write(dir.path(), "elsewhere/viapth/__init__.py", "");
        write(dir.path(), "relative/relmod.py", "");
        write(
            &site,
            "paths.pth",
            format!(
                "# a comment\n{}\n../../../../relative\nimport hook; hook.install()\n",
                dir.path().join("elsewhere").display()
            ),
        );
        write(dir.path(), "pp/onpath.py", "");
        let env = EnvironmentFingerprint {
            interpreter: python,
            framework_args: Vec::new(),
            variables: ProcessEnv::from_vars([("PYTHONPATH", dir.path().join("pp"))]),
        };
        let names = env.installed_modules(dir.path());
        for name in [
            "pkg",
            "single",
            "ext",
            "winext",
            "Some_Dist",
            "some_dist",
            "toplevel",
            "old",
            "oldmod",
            "viapth",
            "relmod",
            "onpath",
        ] {
            assert!(names.contains(name), "{name}: {names:?}");
        }
        for name in ["notes", "paths", "hook", "1"] {
            assert!(!names.contains(name), "{name}: {names:?}");
        }
        let missing = EnvironmentFingerprint {
            interpreter: dir.path().join("nope/bin/python"),
            ..env
        };
        assert!(missing.installed_modules(dir.path()).is_empty());
    }

    /// Whether the interpreter installs a module (pytest): `None` when its packages can't be found, and a venv that
    /// leaves out the system's packages counts only its own.
    #[test]
    fn whether_the_interpreter_installs_a_module() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // The base interpreter, with pytest.
        write(root, "base/bin/python3", "");
        write(
            root,
            "base/lib/python3.13/site-packages/_pytest/__init__.py",
            "",
        );
        write(
            root,
            "base/lib/python3.13/site-packages/pytest-9.1.1.dist-info/METADATA",
            "",
        );
        let base = EnvironmentFingerprint {
            interpreter: root.join("base/bin/python3"),
            ..EnvironmentFingerprint::default()
        };
        assert_eq!(base.installs(root, &["pytest"]), Some(true));
        assert_eq!(base.installs(root, &["_pytest"]), Some(true));
        assert_eq!(base.installs(root, &["nose"]), Some(false));

        // A venv of it, as `python -m venv` makes one: the interpreter a link to
        // the base's.
        write(
            root,
            "venv/pyvenv.cfg",
            format!("home = {}\n", root.join("base/bin").display()),
        );
        fs::create_dir_all(root.join("venv/lib/python3.13/site-packages")).unwrap();
        fs::create_dir_all(root.join("venv/bin")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("base/bin/python3"), root.join("venv/bin/python"))
            .unwrap();
        #[cfg(not(unix))]
        write(root, "venv/bin/python", "");
        let venv = EnvironmentFingerprint {
            interpreter: root.join("venv/bin/python"),
            ..EnvironmentFingerprint::default()
        };
        assert_eq!(venv.installs(root, &["pytest", "_pytest"]), Some(false));
        write(
            root,
            "venv/lib/python3.13/site-packages/pytest/__init__.py",
            "",
        );
        assert_eq!(venv.installs(root, &["pytest", "_pytest"]), Some(true));
        fs::remove_dir_all(root.join("venv/lib/python3.13/site-packages/pytest")).unwrap();
        // One that includes the system's packages sees the base's.
        write(
            root,
            "venv/pyvenv.cfg",
            format!(
                "home = {}\ninclude-system-site-packages = true\n",
                root.join("base/bin").display()
            ),
        );
        #[cfg(unix)]
        assert_eq!(venv.installs(root, &["pytest", "_pytest"]), Some(true));

        // Windows: `Scripts\python.exe` in a venv, `python.exe` beside `Lib` in
        // an installation.
        write(root, "win/python.exe", "");
        write(root, "win/Lib/site-packages/pytest/__init__.py", "");
        let windows = EnvironmentFingerprint {
            interpreter: root.join("win/python.exe"),
            ..EnvironmentFingerprint::default()
        };
        assert_eq!(windows.installs(root, &["pytest"]), Some(true));
        write(root, "winvenv/pyvenv.cfg", "home = C:\\Python313\n");
        write(root, "winvenv/Scripts/python.exe", "");
        fs::create_dir_all(root.join("winvenv/Lib/site-packages")).unwrap();
        let windows = EnvironmentFingerprint {
            interpreter: root.join("winvenv/Scripts/python.exe"),
            ..EnvironmentFingerprint::default()
        };
        assert_eq!(windows.installs(root, &["pytest"]), Some(false));

        // Can't tell: a missing one, a shim with no packages.
        let missing = EnvironmentFingerprint {
            interpreter: root.join("nope/bin/python"),
            ..EnvironmentFingerprint::default()
        };
        assert_eq!(missing.installs(root, &["pytest"]), None);
        write(root, "shims/python", "#!/bin/sh\n");
        let shim = EnvironmentFingerprint {
            interpreter: root.join("shims/python"),
            ..EnvironmentFingerprint::default()
        };
        assert_eq!(shim.installs(root, &["pytest"]), None);
    }
}
