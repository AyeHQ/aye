//! What the run's plan takes from the command line and collection beside
//! the cases: the ids with parameters the command line names
//! ([`requested_ids`]) and, by adapter, the session's files
//! ([`session_files`]).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use aye_events::{AdapterName, Case, ProjectPath, TestId};
use aye_fs::normalize_absolute;

use crate::positional::Positional;

/// The ids with parameters the command line names (`path::test[x]`),
/// relative to the root as collection spells them, each once
/// ([`aye_protocol::BatchContext::requested_ids`]).
pub(crate) fn requested_ids(positionals: &[Positional]) -> Vec<TestId> {
    let mut seen = HashSet::new();
    positionals
        .iter()
        .filter(|p| p.id && p.path.is_some())
        .filter(|p| {
            p.selector
                .split_once("::")
                .is_some_and(|(_, rest)| rest.contains('['))
        })
        .map(|p| p.selector.clone())
        .filter(|id| seen.insert(id.clone()))
        .map(TestId::new)
        .collect()
}

/// By adapter, the files collection found its tests in (selected or not),
/// in collection order: the session every worker of an adapter whose
/// executor lists the session lists before its first batch
/// ([`aye_runner::Plan::session_files`]). Read from the cases, so a
/// claimed file with none (one that failed to import) is left out. With
/// dotted names or keys among the arguments, collection walked the whole
/// project to resolve them, but the user's run is `pytest <the paths it
/// starts from>` ([`super::reach::Reach::selected`], absolute, with the
/// root, as `within`): only the files in those paths are its session, and
/// listing the others would load conftests that run never loads (a
/// `test*` directory's, whose hooks reach every test).
pub(crate) fn session_files(
    cases: &[Case],
    within: Option<(&[PathBuf], &Path)>,
) -> BTreeMap<AdapterName, Vec<ProjectPath>> {
    let mut files: BTreeMap<AdapterName, Vec<ProjectPath>> = BTreeMap::new();
    let mut seen: HashSet<(&AdapterName, &str)> = HashSet::new();
    for case in cases {
        let path = case.id.path();
        let reached = within.is_none_or(|(paths, root)| {
            let file = normalize_absolute(&root.join(path.as_str()));
            paths.iter().any(|start| file.starts_with(start))
        });
        if reached && seen.insert((&case.adapter, path.as_str())) {
            files
                .entry(case.adapter.clone())
                .or_default()
                .push(path.to_project_path());
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::positional;

    #[test]
    fn ids_with_parameters_on_the_command_line_go_to_the_adapters() {
        let root = Path::new("/repo");
        let cwd = Path::new("/repo/tests");
        let positionals: Vec<Positional> = [
            "test_p.py::test_dyn[9]",
            "/repo/tests/test_p.py::TestK::test_m[a-b]",
            "test_p.py::test_plain",
            "test_p.py",
            "pkg.mod.test_x[1]",
            "../tests/test_p.py::test_dyn[9]",
        ]
        .iter()
        .map(|arg| positional::resolve(cwd, root, arg))
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

    #[test]
    fn the_session_is_every_file_with_a_case_by_adapter_in_collection_order() {
        let cases: Vec<Case> = [
            ("b.py::test_x", "pytest"),
            ("b.py::test_y", "pytest"),
            ("b.py::Doc", "doctest"),
            ("a.py::test_x", "pytest"),
            ("b.py::test_z", "pytest"),
            ("c.txt::c.txt", "doctest"),
        ]
        .into_iter()
        .map(|(id, adapter)| Case::new(id, adapter))
        .collect();
        let files = session_files(&cases, None);
        assert_eq!(files.len(), 2);
        assert_eq!(files["pytest"], ["b.py", "a.py"]);
        assert_eq!(files["doctest"], ["b.py", "c.txt"]);
        assert!(session_files(&[], None).is_empty());
    }

    /// With dotted names or keys, collection walks the whole project, but the
    /// session is what `pytest <the paths the run starts from>` collects: a
    /// file elsewhere (whose directory's conftest that run never loads) isn't
    /// listed.
    #[test]
    fn with_dotted_names_the_session_is_what_the_run_starts_from() {
        let root = Path::new("/repo");
        let cases: Vec<Case> = [
            ("testing/test_g.py::test_g", "pytest"),
            ("tests/test_a.py::test_a", "pytest"),
            ("tests/sub/test_s.py::test_s", "pytest"),
            ("tests2/test_t.py::test_t", "pytest"),
            ("lib.py::lib.f", "doctest"),
        ]
        .into_iter()
        .map(|(id, adapter)| Case::new(id, adapter))
        .collect();
        let starts = [root.join("tests"), root.join("lib.py")];
        let files = session_files(&cases, Some((&starts, root)));
        assert_eq!(files["pytest"], ["tests/test_a.py", "tests/sub/test_s.py"]);
        assert_eq!(files["doctest"], ["lib.py"]);
    }
}
