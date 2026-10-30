//! The catalogue of environment variables.

/// Every environment variable aye reads by name or sets, each a constant
/// holding its name, with where aye reads or sets it.
pub struct EnvVars;

/// The constants, and [`EnvVars::ALL`] from the same list.
macro_rules! catalogue {
    ($($(#[$doc:meta])* $name:ident;)+) => {
        impl EnvVars {
            $(
                $(#[$doc])*
                pub const $name: &'static str = stringify!($name);
            )+

            /// Every variable above, in the order they are declared.
            pub const ALL: &'static [&'static str] = &[$(Self::$name),+];
        }
    };
}

catalogue! {
    // aye's own.

    /// The interpreter that runs the tests when `--python` isn't given; it
    /// wins over `[tool.aye] python`. A bare name is looked up on `PATH`,
    /// another relative path is relative to the current directory; empty
    /// is unset. Read by the binary.
    AYE_PYTHON;
    /// Set for every worker: its lane, from 0. Set by the runner.
    AYE_WORKER;

    // Set for every worker, for code written against pytest-xdist.

    /// The run's id, which every worker of the run shares, as pytest-xdist's
    /// `testrun_uid`. Set by the binary; read by aye's engine.
    PYTEST_XDIST_TESTRUNUID;
    /// The worker's name, `gw<lane>`, with more than one lane. Set by the
    /// runner; read by aye's engine.
    PYTEST_XDIST_WORKER;
    /// The number of lanes, with more than one. Set by the runner.
    PYTEST_XDIST_WORKER_COUNT;
    /// `utf-8`, so that a worker's output is UTF-8 whatever the locale. Set
    /// by the runner.
    PYTHONIOENCODING;

    // Set inside workers as pytest sets them.

    /// pytest's version while a pytest or doctest session runs. Set by the
    /// pytest and doctest executors.
    PYTEST_VERSION;
    /// The test running and its phase, `<node id> (call)`. Set by aye's
    /// engine for the pytest executor, and by the doctest executor.
    PYTEST_CURRENT_TEST;

    // The platform's.

    /// Where a bare interpreter name is looked up, and `python3` or
    /// `python` when nothing else chooses the interpreter. Read by the
    /// binary, for aye-provision's discovery.
    PATH;
    /// An active virtual environment, whose interpreter runs the tests when
    /// `--python`, `AYE_PYTHON` and `[tool.aye] python` don't choose one.
    /// Read by the binary, for aye-provision's discovery.
    VIRTUAL_ENV;
    /// The home directory: `~` in `--junitxml`, and in pytest's paths
    /// outside Windows; where the user's own `site-packages` are, for the
    /// refinement cache's key. Read by the binary, aye-provision and the
    /// pytest adapter.
    HOME;
    /// The home directory where `HOME` isn't set, and on Windows in
    /// pytest's paths, as `ntpath.expanduser` reads it. Read by the binary,
    /// aye-provision and the pytest adapter.
    USERPROFILE;
    /// On Windows, the home directory's drive where `USERPROFILE` isn't
    /// set, as `ntpath.expanduser` reads it. Read by the pytest adapter.
    HOMEDRIVE;
    /// On Windows, the home directory's path where `USERPROFILE` isn't set,
    /// as `ntpath.expanduser` reads it. Read by the pytest adapter.
    HOMEPATH;
    /// On Windows, the user's name, for `~user` in pytest's paths, as
    /// `ntpath.expanduser` reads it. Read by the pytest adapter.
    USERNAME;
    /// Where Python imports from beside the standard library: the modules
    /// there count as installed for the refinement cache, and the pytest
    /// adapter finds project files there. Read by aye-provision and the
    /// pytest adapter.
    PYTHONPATH;
    /// Python's `-O`: at 2, docstrings are stripped, so they hold no
    /// doctest. Read by the doctest adapter.
    PYTHONOPTIMIZE;
    /// `true` in a GitHub Actions step, where aye adds annotations to its
    /// output. Read by the binary.
    GITHUB_ACTIONS;
    /// The repository's checkout in a GitHub Actions step: annotations'
    /// paths are relative to it. Read by the binary.
    GITHUB_WORKSPACE;
    /// Turns colour off under `--color auto`. Read by `anstream`, for
    /// aye-render.
    NO_COLOR;
    /// Turns colour on under `--color auto`, off a terminal too. Read by
    /// `anstream`, for aye-render.
    CLICOLOR_FORCE;
    /// Set on a continuous integration service: a failed comparison's
    /// explanation is shown whole, as pytest shows it there. Read by aye's
    /// engine.
    CI;
    /// Set on some continuous integration services (Jenkins), as `CI` is.
    /// Read by aye's engine.
    BUILD_NUMBER;

    // pytest's, read as pytest reads them.

    /// More of pytest's arguments, after the configuration's `addopts`.
    /// Read by the pytest and doctest adapters and the pytest front-end.
    PYTEST_ADDOPTS;
    /// Plugins to load, comma-separated. Read by the pytest and doctest
    /// adapters and the pytest front-end.
    PYTEST_PLUGINS;
    /// Set: no plugin is loaded from an entry point. Read by the pytest
    /// front-end.
    PYTEST_DISABLE_PLUGIN_AUTOLOAD;
    /// `1`: a module imported from another file than the one collected is
    /// no error. Read by the doctest adapter, the doctest executor and the
    /// pytest front-end.
    PY_IGNORE_IMPORTMISMATCH;
    /// Where `tmp_path` and `tmp_path_factory` make their directories,
    /// rather than the system's temporary directory. Read by aye's engine.
    PYTEST_DEBUG_TEMPROOT;
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::EnvVars;

    #[test]
    fn every_name_is_listed_once_and_is_its_own_constant() {
        let names: BTreeSet<&str> = EnvVars::ALL.iter().copied().collect();
        assert_eq!(names.len(), EnvVars::ALL.len());
        assert!(names.iter().all(|name| {
            name.len() > 1
                && name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        }));
        assert_eq!(EnvVars::PYTEST_ADDOPTS, "PYTEST_ADDOPTS");
        assert!(names.contains(EnvVars::AYE_PYTHON));
    }
}
