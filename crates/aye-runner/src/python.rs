//! Choosing the interpreter that runs the tests.

use std::env;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

/// Where the interpreter came from, for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonSource {
    /// `--python`.
    Flag,
    /// `$AYE_PYTHON`.
    Env,
    /// `[tool.aye] python`.
    Config,
    /// An active `$VIRTUAL_ENV`.
    VirtualEnv,
    /// A `.venv` in the current directory or the project root.
    ProjectVenv,
    /// The interpreter installed beside the `aye` binary.
    BesideAye,
    /// `python3` or `python` on `PATH`.
    Path,
}

impl fmt::Display for PythonSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Flag => "--python",
            Self::Env => "$AYE_PYTHON",
            Self::Config => "[tool.aye] python",
            Self::VirtualEnv => "$VIRTUAL_ENV",
            Self::ProjectVenv => ".venv",
            Self::BesideAye => "beside aye",
            Self::Path => "PATH",
        })
    }
}

/// Find the Python interpreter, in priority order: `--python`, `$AYE_PYTHON`,
/// `[tool.aye] python`, an active `$VIRTUAL_ENV`, a `.venv` in `cwd` or
/// `root`, the interpreter installed beside `aye` (beside the file a link
/// to it leads to), then `python3` or `python` on `PATH`.
///
/// An explicit choice (the first three) is returned even when it doesn't
/// exist, so that starting it fails with its name. A bare name (`python3.12`)
/// is looked up on `PATH`; any other relative path is relative to `cwd`, or
/// to `root` when it comes from the config.
pub fn find_python(
    flag: Option<&Path>,
    config: Option<&str>,
    root: &Path,
    cwd: &Path,
) -> Option<(PathBuf, PythonSource)> {
    let system = System {
        var: &|name| env::var_os(name),
        exe: env::current_exe().ok(),
    };
    find(&system, flag, config, root, cwd)
}

/// What discovery reads from the machine, so tests can supply it.
struct System<'a> {
    var: &'a dyn Fn(&str) -> Option<OsString>,
    exe: Option<PathBuf>,
}

impl System<'_> {
    fn var(&self, name: &str) -> Option<OsString> {
        (self.var)(name).filter(|value| !value.is_empty())
    }

    fn which(&self, name: &str) -> Option<PathBuf> {
        let path = self.var("PATH")?;
        env::split_paths(&path)
            .map(|dir| dir.join(exe_name(name)))
            .find(|candidate| candidate.is_file())
    }

    /// A user-given interpreter: a bare name is looked up on `PATH`, a
    /// relative path is joined to `base`.
    fn resolve(&self, given: &Path, base: &Path) -> PathBuf {
        let is_name = given.components().count() == 1 && !given.has_root();
        if is_name && let Some(name) = given.to_str() {
            return self.which(name).unwrap_or_else(|| given.to_path_buf());
        }
        base.join(given)
    }
}

fn find(
    system: &System<'_>,
    flag: Option<&Path>,
    config: Option<&str>,
    root: &Path,
    cwd: &Path,
) -> Option<(PathBuf, PythonSource)> {
    if let Some(path) = flag {
        return Some((system.resolve(path, cwd), PythonSource::Flag));
    }
    if let Some(path) = system.var("AYE_PYTHON") {
        return Some((system.resolve(Path::new(&path), cwd), PythonSource::Env));
    }
    if let Some(path) = config.filter(|path| !path.is_empty()) {
        return Some((system.resolve(Path::new(path), root), PythonSource::Config));
    }
    if let Some(venv) = system.var("VIRTUAL_ENV")
        && let Some(python) = venv_python(Path::new(&venv))
    {
        return Some((python, PythonSource::VirtualEnv));
    }
    for dir in [cwd, root] {
        if let Some(python) = venv_python(&dir.join(".venv")) {
            return Some((python, PythonSource::ProjectVenv));
        }
    }
    let exe = system.exe.as_deref().map(followed);
    if let Some(bin) = exe.as_deref().and_then(Path::parent) {
        for name in ["python", "python3"] {
            let candidate = bin.join(exe_name(name));
            if candidate.is_file() {
                return Some((candidate, PythonSource::BesideAye));
            }
        }
    }
    ["python3", "python"]
        .into_iter()
        .find_map(|name| system.which(name))
        .map(|python| (python, PythonSource::Path))
}

/// The file `path` leads to, when it is a symbolic link: `uv tool install`
/// and pipx link the binary into `~/.local/bin` from the tool's
/// environment, which holds its interpreter. macOS's `current_exe` returns
/// the path as invoked; Linux's has followed the links already. Any other
/// path stays as it is.
fn followed(path: &Path) -> PathBuf {
    let is_link = std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink());
    if is_link && let Ok(target) = std::fs::canonicalize(path) {
        return target;
    }
    path.to_path_buf()
}

/// The interpreter of a virtual environment, if it has one.
pub fn venv_python(venv: &Path) -> Option<PathBuf> {
    let python = if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    };
    python.is_file().then_some(python)
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) && Path::new(name).extension().is_none() {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use super::*;

    struct Machine {
        dir: tempfile::TempDir,
        vars: HashMap<&'static str, OsString>,
        exe: Option<PathBuf>,
    }

    impl Machine {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
                vars: HashMap::new(),
                exe: None,
            }
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.dir.path().join(rel)
        }

        /// Create an empty file standing in for an interpreter.
        fn touch(&self, rel: &str) -> PathBuf {
            let path = self.path(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "").unwrap();
            path
        }

        fn venv(&self, rel: &str) -> PathBuf {
            if cfg!(windows) {
                self.touch(&format!("{rel}/Scripts/python.exe"))
            } else {
                self.touch(&format!("{rel}/bin/python"))
            }
        }

        fn find(
            &self,
            flag: Option<&Path>,
            config: Option<&str>,
        ) -> Option<(PathBuf, PythonSource)> {
            let var = |name: &str| self.vars.get(name).cloned();
            let system = System {
                var: &var,
                exe: self.exe.clone(),
            };
            find(
                &system,
                flag,
                config,
                &self.path("root"),
                &self.path("root/sub"),
            )
        }
    }

    #[test]
    fn explicit_choices_win_in_order() {
        let mut m = Machine::new();
        m.venv("root/.venv");
        m.vars.insert("AYE_PYTHON", "env/python".into());
        assert_eq!(
            m.find(Some(Path::new("flag/python")), Some("cfg/python")),
            Some((m.path("root/sub/flag/python"), PythonSource::Flag))
        );
        assert_eq!(
            m.find(None, Some("cfg/python")),
            Some((m.path("root/sub/env/python"), PythonSource::Env))
        );
        m.vars.remove("AYE_PYTHON");
        assert_eq!(
            m.find(None, Some("cfg/python")),
            Some((m.path("root/cfg/python"), PythonSource::Config))
        );
    }

    #[test]
    fn bare_names_are_looked_up_on_path() {
        let mut m = Machine::new();
        let found = m.touch(&format!("bin/{}", exe_name("python3.12")));
        m.vars.insert("PATH", m.path("bin").into_os_string());
        assert_eq!(
            m.find(Some(Path::new("python3.12")), None),
            Some((found, PythonSource::Flag))
        );
        assert_eq!(
            m.find(None, Some("python3.99")),
            Some((PathBuf::from("python3.99"), PythonSource::Config))
        );
    }

    #[test]
    fn environments_before_path() {
        let mut m = Machine::new();
        let on_path = m.touch(&format!("bin/{}", exe_name("python3")));
        m.vars.insert("PATH", m.path("bin").into_os_string());
        assert_eq!(m.find(None, None), Some((on_path, PythonSource::Path)));

        let beside = m.touch(&format!("aye/bin/{}", exe_name("python")));
        m.exe = Some(m.path("aye/bin/aye"));
        assert_eq!(m.find(None, None), Some((beside, PythonSource::BesideAye)));

        let project = m.venv("root/.venv");
        assert_eq!(
            m.find(None, None),
            Some((project, PythonSource::ProjectVenv))
        );
        let here = m.venv("root/sub/.venv");
        assert_eq!(m.find(None, None), Some((here, PythonSource::ProjectVenv)));

        let active = m.venv("active");
        m.vars
            .insert("VIRTUAL_ENV", m.path("active").into_os_string());
        assert_eq!(m.find(None, None), Some((active, PythonSource::VirtualEnv)));
    }

    /// `uv tool install` and pipx link the binary into `~/.local/bin`,
    /// which may hold an interpreter of its own (`uv python install
    /// --default`); the tool's is beside the file the link leads to.
    #[cfg(unix)]
    #[test]
    fn the_interpreter_beside_aye_is_found_through_links_to_it() {
        let mut m = Machine::new();
        m.touch("tools/aye/bin/aye");
        let tool = m.touch("tools/aye/bin/python");
        m.touch("local/bin/python");
        fs::create_dir_all(m.path("links")).unwrap();
        std::os::unix::fs::symlink("../tools/aye/bin/aye", m.path("links/aye")).unwrap();
        std::os::unix::fs::symlink(m.path("links/aye"), m.path("local/bin/aye")).unwrap();
        m.exe = Some(m.path("local/bin/aye"));
        assert_eq!(
            m.find(None, None),
            Some((tool.canonicalize().unwrap(), PythonSource::BesideAye))
        );
        // A link that leads nowhere is looked beside, as it is.
        std::os::unix::fs::symlink("loop", m.path("local/bin/loop")).unwrap();
        m.exe = Some(m.path("local/bin/loop"));
        assert_eq!(
            m.find(None, None),
            Some((m.path("local/bin/python"), PythonSource::BesideAye))
        );
    }

    #[test]
    fn an_empty_variable_counts_as_unset() {
        let mut m = Machine::new();
        m.vars.insert("AYE_PYTHON", OsString::new());
        m.vars.insert("VIRTUAL_ENV", OsString::new());
        assert_eq!(m.find(None, None), None);
    }

    #[test]
    fn a_virtualenv_without_python_is_skipped() {
        let mut m = Machine::new();
        fs::create_dir_all(m.path("broken")).unwrap();
        m.vars
            .insert("VIRTUAL_ENV", m.path("broken").into_os_string());
        let project = m.venv("root/.venv");
        assert_eq!(
            m.find(None, None),
            Some((project, PythonSource::ProjectVenv))
        );
    }
}
