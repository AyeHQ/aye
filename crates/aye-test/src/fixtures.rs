//! The projects tests run aye on: one written for a test ([`project`]), a
//! copy of one ([`copy_project`]), the examples ([`examples_dir`]); where a
//! project's runs are kept ([`runs_dir`]); and an interpreter without
//! pytest ([`venv_without_pytest`]).

use std::fs;
use std::path::{Path, PathBuf};

use aye_fs::{StateBucket, StateDir, normalize_absolute};

/// What a copy of a project leaves out: what runs and tools leave in it,
/// and a virtual environment.
const LEFT_OUT: [&str; 4] = [StateDir::NAME, "__pycache__", ".pytest_cache", ".venv"];

/// The workspace's root, where its `.venv` and `examples/` are.
pub fn workspace_root() -> PathBuf {
    normalize_absolute(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

/// The example projects, one per framework, each passing under aye and
/// under its own runner. A packaged crate has none beside it.
pub fn examples_dir() -> PathBuf {
    workspace_root().join("examples")
}

/// A project in a fresh directory: `files` are (path, contents), and a
/// `pyproject.toml` is added at the top unless one is given.
pub fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    if !files.iter().any(|(path, _)| *path == "pyproject.toml") {
        fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"p\"\nversion = \"0\"\n",
        )
        .unwrap();
    }
    for (path, contents) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    dir
}

/// Copies the project at `from` to `to`, so that runs never share history
/// or caches: without aye's state directory, `__pycache__`,
/// `.pytest_cache`, a `.venv`, or the entries `leaving_out` names.
pub fn copy_project(from: &Path, to: &Path, leaving_out: &[&str]) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if LEFT_OUT.iter().chain(leaving_out).any(|left| name == *left) {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_project(&entry.path(), &target, leaving_out);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Where the project rooted at `root` keeps its runs (`.aye/runs`), which
/// aye-history's run store reads.
pub fn runs_dir(root: &Path) -> PathBuf {
    StateDir::for_root(root).bucket(StateBucket::Runs)
}

/// A venv of `python`'s base interpreter without pytest, in `dir`, laid
/// out as `python -m venv` lays one out: an interpreter beside a
/// `pyvenv.cfg` leaving out the system's packages, and an empty
/// `site-packages`. `None` when it can't be made, and, saying so on
/// stderr, when the base interpreter itself has pytest.
#[cfg(unix)]
#[expect(clippy::print_stderr, reason = "a test says why it skips")]
pub fn venv_without_pytest(python: &Path, dir: &Path) -> Option<PathBuf> {
    use std::process::{Command, Stdio};

    let probe = Command::new(python)
        .args([
            "-c",
            "import sys; print(sys._base_executable); print(f'{sys.version_info[0]}.{sys.version_info[1]}')",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8(probe.stdout).ok()?;
    let mut lines = text.lines();
    let base = PathBuf::from(lines.next()?);
    let version = lines.next()?;
    let venv = dir.join("venv");
    fs::create_dir_all(venv.join("bin")).ok()?;
    fs::create_dir_all(venv.join(format!("lib/python{version}/site-packages"))).ok()?;
    fs::write(
        venv.join("pyvenv.cfg"),
        format!(
            "home = {}\ninclude-system-site-packages = false\nversion = {version}\n",
            base.parent()?.display()
        ),
    )
    .ok()?;
    let interpreter = venv.join("bin/python");
    std::os::unix::fs::symlink(&base, &interpreter).ok()?;
    let has_pytest = Command::new(&interpreter)
        .args(["-c", "import pytest"])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if has_pytest {
        eprintln!(
            "no venv without pytest: the base interpreter {} has pytest",
            base.display()
        );
        return None;
    }
    Some(interpreter)
}

/// A venv without pytest, outside Unix: none is made.
#[cfg(not(unix))]
pub fn venv_without_pytest(_python: &Path, _dir: &Path) -> Option<PathBuf> {
    None
}
