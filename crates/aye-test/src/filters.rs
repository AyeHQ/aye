//! What snapshots of aye's output leave out ([`filters`]).

use std::fs;
use std::path::Path;

/// What varies between machines and runs in aye's output, as (regex,
/// replacement) pairs, in the order they apply: `root`'s path, resolved
/// then as given (`[ROOT]`), the interpreter's path (`[PYTHON]`) and
/// version, durations, pids, run ids and aye's version.
pub fn filters(root: Option<&Path>, python: Option<&Path>) -> Vec<(String, String)> {
    let mut filters = Vec::new();
    if let Some(root) = root {
        let resolved = fs::canonicalize(root).unwrap();
        for path in [resolved.as_path(), root] {
            filters.push((regex_escape(path.to_str().unwrap()), "[ROOT]".to_owned()));
        }
    }
    if let Some(python) = python {
        filters.push((
            regex_escape(python.to_str().unwrap()),
            "[PYTHON]".to_owned(),
        ));
    }
    for (pattern, replacement) in [
        (r"python \d+\.\d+\.\d+\S*", "python [VERSION]"),
        (r#""python":"\d+\.\d+[^"]*""#, r#""python":"[VERSION]""#),
        (r#""version":"\d+\.\d+[^"]*""#, r#""version":"[VERSION]""#),
        (r#""aye_version":"[^"]*""#, r#""aye_version":"[VERSION]""#),
        (
            r#""(t_ms|duration_ms|body_duration_ms|ms|s)":[-0-9.eE+]+"#,
            r#""$1":[N]"#,
        ),
        (r"\b\d+(\.\d+)? ?(ms|s)\b", "[TIME]"),
        (r#""pid":\d+"#, r#""pid":[PID]"#),
        (r"\bpid \d+", "pid [PID]"),
        (r#""run":"[0-9a-f]{16}""#, r#""run":"[RUN]""#),
    ] {
        filters.push((pattern.to_owned(), replacement.to_owned()));
    }
    filters
}

/// `text`, matched literally by a regex.
fn regex_escape(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            let special = "\\.+*?()|[]{}^$#&-~".contains(c);
            special.then_some('\\').into_iter().chain([c])
        })
        .collect()
}
