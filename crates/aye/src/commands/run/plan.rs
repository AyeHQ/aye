//! The plan a run hands the runner ([`plan`]): the cases selected, each
//! batch's policy and each adapter's context, with what every batch's
//! context gets beside them (the ids with parameters the command line
//! names, [`requested_ids`]; the reruns' options; the paths the run starts
//! from), history's durations, what runs first under `--ff`, and, by
//! adapter, the session's files
//! ([`aye_collect::Collection::session_files`]).
//!
//! Decided (implementation, October 2026):
//! - **Ids with parameters on the command line** (`path::test[x]`) go to
//!   every adapter's context as `requested_ids`, relative to the root.
//!   Where collection only knows the test without its parameters
//!   (`--no-refine`, a failed refinement), the batch gets the typed id, and
//!   the framework reports it `not-found` when it makes no such test rather
//!   than run the test's other parameters in its place, as it may for ids
//!   aye worked out itself.
//! - **Reruns repeat aye's options that shape the run.** `-c FILE` decides
//!   which tests exist and how they run, so every adapter's context of a
//!   batch gets `rerun_args` (`["-c", FILE]`, as given, for the directory
//!   aye ran in), which adapters put before the id: `aye -c FILE <id> --
//!   <args>`. `--python` is left out (a rerun finds the interpreter as aye
//!   did), and so are the options that shape only the output or the
//!   selection, which a rerun of one id doesn't need, and `--junitxml`,
//!   whose report it would overwrite.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use aye_events::TestId;
use aye_runner::Plan;

use super::select::Selected;
use crate::positional::Positionals;
use crate::settings::RunSettings;

/// The plan of the run `settings` ask for, of the cases `selected`, with
/// history's `durations` and, under `--ff`, the tests that failed last
/// (`last_failed`) to run first.
pub(super) fn plan(
    settings: &RunSettings,
    selected: Selected,
    durations: HashMap<TestId, Duration>,
    last_failed: HashSet<TestId>,
) -> Plan {
    Plan {
        cases: selected.cases,
        policy: settings.policy.clone(),
        contexts: settings
            .adapters
            .prepared()
            .iter()
            .map(|(name, r)| (name.clone(), r.context()))
            .collect(),
        requested_ids: requested_ids(&settings.positionals),
        rerun_args: settings.rerun_args.clone(),
        selected_paths: selected
            .reach
            .map(|reach| reach.selected)
            .unwrap_or_default()
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        durations,
        first: if settings.failed_first {
            last_failed
        } else {
            HashSet::new()
        },
        session_files: selected.session_files,
    }
}

/// The ids with parameters the command line names (`path::test[x]`),
/// relative to the root as collection spells them, each once
/// ([`aye_protocol::BatchContext::requested_ids`]).
fn requested_ids(positionals: &Positionals) -> Vec<TestId> {
    let mut seen = HashSet::new();
    positionals
        .iter()
        .filter(|p| p.has_params())
        .map(|p| p.selector().to_owned())
        .filter(|id| seen.insert(id.clone()))
        .map(TestId::new)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::positional::Positional;

    #[test]
    fn ids_with_parameters_on_the_command_line_go_to_the_adapters() {
        let root = Path::new("/repo");
        let cwd = Path::new("/repo/tests");
        let positionals: Positionals = [
            "test_p.py::test_dyn[9]",
            "/repo/tests/test_p.py::TestK::test_m[a-b]",
            "test_p.py::test_plain",
            "test_p.py",
            "pkg.mod.test_x[1]",
            "../tests/test_p.py::test_dyn[9]",
        ]
        .iter()
        .map(|arg| Positional::resolve(cwd, root, arg))
        .collect();
        let requested = requested_ids(&positionals);
        assert_eq!(
            requested,
            [
                TestId::new("tests/test_p.py::test_dyn[9]"),
                TestId::new("tests/test_p.py::TestK::test_m[a-b]")
            ]
        );
    }
}
