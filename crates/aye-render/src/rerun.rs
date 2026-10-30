//! The command that reruns a test ([`rerun`]) or a file's tests
//! ([`rerun_path`]), quoted for a shell.

use aye_events::{Reproduce, TestId};

/// The command that reruns a test: the recorded one, or `aye <id>`.
pub(crate) fn rerun(id: &TestId, reproduce: Option<&Reproduce>) -> String {
    match reproduce {
        Some(reproduce) if !reproduce.command.is_empty() => join(&reproduce.command),
        _ => join(&["aye", id.as_str()]),
    }
}

/// The command that reruns a file's tests: `aye <path>`.
pub(crate) fn rerun_path(path: &str) -> String {
    join(&["aye", path])
}

/// Words joined into one shell command, quoted where a shell needs it.
fn join<S: AsRef<str>>(words: &[S]) -> String {
    shlex::try_join(words.iter().map(AsRef::as_ref)).unwrap_or_else(|_| {
        words
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>()
            .join(" ")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rerun_quotes_for_the_shell() {
        let id = TestId::new("tests/t.py::test_x[1-2]");
        assert_eq!(rerun(&id, None), "aye 'tests/t.py::test_x[1-2]'");
        let reproduce = Reproduce {
            command: vec!["aye".into(), "tests/t.py::test_y".into(), "--seed=7".into()],
            inputs: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            rerun(&id, Some(&reproduce)),
            "aye tests/t.py::test_y '--seed=7'"
        );
    }
}
