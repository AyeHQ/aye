//! Finding the interpreter that runs the tests ([`find`]): the one asked
//! for, else the first an environment, the project or the machine offers.

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::request::{PythonRequest, RequestSource};

/// Where the interpreter came from: asked for, or found. `aye config` shows
/// it as the binary words it: the request's source, or the rule discovery
/// followed (`.venv`, `beside aye`, `PATH`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonSource {
    /// Asked for: by `--python`, `$AYE_PYTHON` or `[tool.aye] python`.
    Requested(RequestSource),
    /// An active `$VIRTUAL_ENV`.
    VirtualEnv,
    /// A `.venv` in the current directory or the project root.
    ProjectVenv(VenvDir),
    /// The interpreter installed beside the `aye` binary.
    BesideAye,
    /// `python3` or `python` on `PATH`.
    SearchPath,
}

/// Where a project's `.venv` was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenvDir {
    /// The current directory.
    Cwd,
    /// The project root.
    Root,
}

/// The interpreter that runs the tests, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interpreter {
    /// Absolute, or as a request or `PATH` gave it.
    pub executable: PathBuf,
    pub source: PythonSource,
}

/// Nothing asked for an interpreter, and none was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no Python interpreter found: pass --python, or set [tool.aye] python")]
pub struct PythonNotFound;

/// What discovery reads beside the files: two variables of the environment
/// aye started in, where an empty value counts as unset, and the running
/// `aye`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveryEnv {
    /// `$VIRTUAL_ENV`: an active virtual environment.
    pub virtual_env: Option<OsString>,
    /// `$PATH`: where names are looked up.
    pub path: Option<OsString>,
    /// The running `aye` (`std::env::current_exe`), as invoked.
    pub current_exe: Option<PathBuf>,
}

impl DiscoveryEnv {
    fn var(value: Option<&OsString>) -> Option<&OsStr> {
        value
            .map(OsString::as_os_str)
            .filter(|value| !value.is_empty())
    }

    /// The first file named `name` in a directory of `PATH`.
    fn which(&self, name: &str) -> Option<PathBuf> {
        let path = Self::var(self.path.as_ref())?;
        env::split_paths(path)
            .map(|dir| dir.join(exe_name(name)))
            .find(|candidate| candidate.is_file())
    }
}

/// The interpreter that runs the tests: the one `request` asks for, else
/// the first of an active `$VIRTUAL_ENV`, a `.venv` in `cwd` then `root`,
/// the interpreter installed beside `aye` (beside the file a link to it
/// leads to), then `python3` or `python` on `PATH`.
///
/// A request is answered even when its file doesn't exist, so that starting
/// it fails with its name, and a name `PATH` doesn't hold is answered as it
/// was given.
pub fn find(
    request: Option<(PythonRequest, RequestSource)>,
    env: &DiscoveryEnv,
    root: &Path,
    cwd: &Path,
) -> Result<Interpreter, PythonNotFound> {
    let found = |executable, source| Ok(Interpreter { executable, source });
    if let Some((request, source)) = request {
        let executable = match request {
            PythonRequest::File(path) => path,
            PythonRequest::ExecutableName(name) => {
                env.which(&name).unwrap_or_else(|| PathBuf::from(name))
            }
        };
        return found(executable, PythonSource::Requested(source));
    }
    if let Some(venv) = DiscoveryEnv::var(env.virtual_env.as_ref())
        && let Some(python) = venv_python(Path::new(venv))
    {
        return found(python, PythonSource::VirtualEnv);
    }
    for (dir, which) in [(cwd, VenvDir::Cwd), (root, VenvDir::Root)] {
        if let Some(python) = venv_python(&dir.join(".venv")) {
            return found(python, PythonSource::ProjectVenv(which));
        }
    }
    let exe = env.current_exe.as_deref().map(followed);
    if let Some(bin) = exe.as_deref().and_then(Path::parent) {
        for name in ["python", "python3"] {
            let candidate = bin.join(exe_name(name));
            if candidate.is_file() {
                return found(candidate, PythonSource::BesideAye);
            }
        }
    }
    ["python3", "python"]
        .into_iter()
        .find_map(|name| env.which(name))
        .map_or(Err(PythonNotFound), |python| {
            found(python, PythonSource::SearchPath)
        })
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
    use std::fs;

    use super::*;

    struct Machine {
        dir: tempfile::TempDir,
        env: DiscoveryEnv,
    }

    impl Machine {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
                env: DiscoveryEnv::default(),
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
            request: Option<(PythonRequest, RequestSource)>,
        ) -> Option<(PathBuf, PythonSource)> {
            find(
                request,
                &self.env,
                &self.path("root"),
                &self.path("root/sub"),
            )
            .ok()
            .map(|found| (found.executable, found.source))
        }
    }

    /// A request is answered as asked, whatever an environment offers.
    #[test]
    fn a_request_wins() {
        let m = Machine::new();
        m.venv("root/.venv");
        let file = PythonRequest::parse(Path::new("flag/python"), &m.path("root/sub"));
        assert_eq!(
            m.find(Some((file, RequestSource::Flag))),
            Some((
                m.path("root/sub/flag/python"),
                PythonSource::Requested(RequestSource::Flag)
            ))
        );
        let file = PythonRequest::parse(Path::new("cfg/python"), &m.path("root"));
        assert_eq!(
            m.find(Some((file, RequestSource::Config))),
            Some((
                m.path("root/cfg/python"),
                PythonSource::Requested(RequestSource::Config)
            ))
        );
    }

    #[test]
    fn bare_names_are_looked_up_on_path() {
        let mut m = Machine::new();
        let found = m.touch(&format!("bin/{}", exe_name("python3.12")));
        m.env.path = Some(m.path("bin").into_os_string());
        let name = |name: &str| PythonRequest::ExecutableName(name.to_string());
        assert_eq!(
            m.find(Some((name("python3.12"), RequestSource::Flag))),
            Some((found, PythonSource::Requested(RequestSource::Flag)))
        );
        assert_eq!(
            m.find(Some((name("python3.99"), RequestSource::Config))),
            Some((
                PathBuf::from("python3.99"),
                PythonSource::Requested(RequestSource::Config)
            ))
        );
    }

    #[test]
    fn environments_before_path() {
        let mut m = Machine::new();
        let on_path = m.touch(&format!("bin/{}", exe_name("python3")));
        m.env.path = Some(m.path("bin").into_os_string());
        assert_eq!(m.find(None), Some((on_path, PythonSource::SearchPath)));

        let beside = m.touch(&format!("aye/bin/{}", exe_name("python")));
        m.env.current_exe = Some(m.path("aye/bin/aye"));
        assert_eq!(m.find(None), Some((beside, PythonSource::BesideAye)));

        let project = m.venv("root/.venv");
        assert_eq!(
            m.find(None),
            Some((project, PythonSource::ProjectVenv(VenvDir::Root)))
        );
        let here = m.venv("root/sub/.venv");
        assert_eq!(
            m.find(None),
            Some((here, PythonSource::ProjectVenv(VenvDir::Cwd)))
        );

        let active = m.venv("active");
        m.env.virtual_env = Some(m.path("active").into_os_string());
        assert_eq!(m.find(None), Some((active, PythonSource::VirtualEnv)));
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
        m.env.current_exe = Some(m.path("local/bin/aye"));
        assert_eq!(
            m.find(None),
            Some((tool.canonicalize().unwrap(), PythonSource::BesideAye))
        );
        // A link that leads nowhere is looked beside, as it is.
        std::os::unix::fs::symlink("loop", m.path("local/bin/loop")).unwrap();
        m.env.current_exe = Some(m.path("local/bin/loop"));
        assert_eq!(
            m.find(None),
            Some((m.path("local/bin/python"), PythonSource::BesideAye))
        );
    }

    #[test]
    fn an_empty_variable_counts_as_unset() {
        let mut m = Machine::new();
        m.env.virtual_env = Some(OsString::new());
        m.env.path = Some(OsString::new());
        assert_eq!(m.find(None), None);
        assert_eq!(
            find(None, &m.env, &m.path("root"), &m.path("root")),
            Err(PythonNotFound)
        );
        assert_eq!(
            PythonNotFound.to_string(),
            "no Python interpreter found: pass --python, or set [tool.aye] python"
        );
    }

    #[test]
    fn a_virtualenv_without_python_is_skipped() {
        let mut m = Machine::new();
        fs::create_dir_all(m.path("broken")).unwrap();
        m.env.virtual_env = Some(m.path("broken").into_os_string());
        let project = m.venv("root/.venv");
        assert_eq!(
            m.find(None),
            Some((project, PythonSource::ProjectVenv(VenvDir::Root)))
        );
    }
}
