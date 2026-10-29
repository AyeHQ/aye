use super::*;

#[test]
fn source_files_turn_ranges_into_spans() {
    let source = "import pytest\n\n\ndef test_x():\n    pass\n";
    let parsed = ruff_python_parser::parse_module(source).unwrap();
    let lines = LineIndex::from_source_text(source);
    let path = Path::new("/repo/t.py");
    let file = SourceFile {
        rel: ProjectPathRef::as_written("t.py"),
        abs: path,
        source,
        module: parsed.syntax(),
        lines: &lines,
    };
    let func = file.module.body[1].as_function_def_stmt().unwrap();
    let span = file.span(func.name.range());
    assert_eq!(span.line, Some(4));
    assert_eq!(file.text(func.name.range()), "test_x");
    assert_eq!((span.start, span.end), (20, 26));
}

/// Claims, as found, what `claims` says of a path, and remembers what it
/// was asked about.
struct Probed {
    claims: fn(&str) -> bool,
    asked: std::sync::Mutex<Vec<(String, Requested)>>,
}

impl Recognizer for Probed {
    fn roots(&self, _: &Project<'_>) -> Vec<PathBuf> {
        Vec::new()
    }

    fn descend(&self, _: &Path) -> bool {
        true
    }

    fn claim(&self, rel: &str, requested: Requested) -> Claim {
        self.asked
            .lock()
            .unwrap()
            .push((rel.to_string(), requested));
        if (self.claims)(rel) {
            Claim::Tests
        } else {
            Claim::Ignore
        }
    }

    fn recognize(&self, _: &SourceFile<'_>, _: Requested, _: &[SourceFile<'_>]) -> Recognition {
        Recognition::default()
    }
}

/// A recognizer claims any name when it claims a file named nothing in
/// particular, found in the directory with the extension.
#[test]
fn claiming_any_name_is_claiming_a_file_named_nothing_in_particular() {
    let modules = Probed {
        claims: |rel| Path::new(rel).extension().is_some_and(|e| e == "py"),
        asked: std::sync::Mutex::default(),
    };
    assert!(modules.claims_any_name("tests/unit", "py"));
    assert!(modules.claims_any_name("", "py"));
    assert!(!modules.claims_any_name("docs", "txt"));
    assert!(!modules.claims_any_name("", ""));
    assert_eq!(
        *modules.asked.lock().unwrap(),
        [
            (
                "tests/unit/__aye_any_name__.py".to_string(),
                Requested::Found
            ),
            ("__aye_any_name__.py".to_string(), Requested::Found),
            ("docs/__aye_any_name__.txt".to_string(), Requested::Found),
            ("__aye_any_name__".to_string(), Requested::Found),
        ]
    );
    let checks = Probed {
        claims: |rel| {
            rel.rsplit('/')
                .next()
                .is_some_and(|n| n.starts_with("test_"))
        },
        asked: std::sync::Mutex::default(),
    };
    assert!(!checks.claims_any_name("tests", "py"));
}
