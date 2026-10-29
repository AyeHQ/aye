//! Explicit selectors: the positional arguments that name tests.

use std::fmt;

use aye_events::{Case, Certainty, TestId};

use crate::Truth;
use crate::expression::is_python_identifier;

/// One positional argument naming tests: a path, an id, a dotted name or a
/// key prefix. See the crate docs for how each is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    text: String,
    readings: Vec<Reading>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Reading {
    /// A file or directory, as path components.
    Path(Vec<String>),
    /// `path::Class::test[params]`, or a prefix of one by whole names.
    Id {
        path: String,
        names: Vec<String>,
        params: Option<String>,
    },
    /// `pkg.mod.Class.test[params]`, or a prefix of one by whole names.
    Dotted {
        names: Vec<String>,
        params: Option<String>,
    },
    /// The start of a test key, lowercase.
    Key(String),
}

impl Selector {
    /// Read a positional argument: an id when it contains `::`, a path when
    /// it contains `/`, and otherwise every way the word can be read.
    pub fn parse(text: &str) -> Self {
        let readings = if let Some((path, rest)) = text.split_once("::") {
            let (names, params) = split_params(rest);
            vec![Reading::Id {
                path: normalize(path).join("/"),
                names: names.split("::").map(str::to_string).collect(),
                params: params.map(str::to_string),
            }]
        } else if text.contains('/') {
            vec![Reading::Path(normalize(text))]
        } else {
            let mut readings = vec![Reading::Path(normalize(text))];
            let (names, params) = split_params(text);
            if names.split('.').all(is_python_identifier) {
                readings.push(Reading::Dotted {
                    names: names.split('.').map(str::to_string).collect(),
                    params: params.map(str::to_string),
                });
            }
            if (4..=16).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_hexdigit()) {
                readings.push(Reading::Key(text.to_ascii_lowercase()));
            }
            readings
        };
        Self {
            text: text.to_string(),
            readings,
        }
    }

    /// A file or directory and nothing else, for callers that know the
    /// argument exists on disk.
    pub fn path(text: &str) -> Self {
        Self {
            text: text.to_string(),
            readings: vec![Reading::Path(normalize(text))],
        }
    }

    /// The argument as given.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// `True` when this selects the whole case, `Unknown` when it selects
    /// only some parameter ids of a `Partial` case (pushed to `narrow`).
    #[cfg(test)]
    pub(crate) fn select(&self, case: &Parts<'_>, narrow: &mut Vec<TestId>) -> Truth {
        self.select_ranked(case, narrow).0
    }

    /// As [`Selector::select`], with how specific the most general reading
    /// that selects the case is: the number of path components, names and
    /// parameter ids it names (a key names a case outright). A more general
    /// selector subsumes a more specific one, as pytest's
    /// `normalize_collection_arguments` has it.
    pub(crate) fn select_ranked(
        &self,
        case: &Parts<'_>,
        narrow: &mut Vec<TestId>,
    ) -> (Truth, usize) {
        let mut truth = Truth::False;
        let mut rank = usize::MAX;
        for reading in &self.readings {
            let found = match reading {
                Reading::Path(components) => Found::from(starts_with(&case.components, components)),
                Reading::Id {
                    path,
                    names,
                    params,
                } if *path == case.path => case.names(names, params.as_deref(), &case.names),
                Reading::Id { .. } => Found::No,
                Reading::Dotted { names, params } => case
                    .dotted
                    .iter()
                    .map(|dotted| case.names(names, params.as_deref(), dotted))
                    .find(|found| !matches!(found, Found::No))
                    .unwrap_or(Found::No),
                Reading::Key(prefix) => Found::from(case.case.id.key().0.starts_with(prefix)),
            };
            if !matches!(found, Found::No) {
                rank = rank.min(reading.rank());
            }
            truth = truth
                | match found {
                    Found::No => Truth::False,
                    Found::Whole => Truth::True,
                    Found::Narrow(id) => {
                        narrow.push(id);
                        Truth::Unknown
                    }
                };
        }
        (truth, rank)
    }

    /// Whether this names the file `path` (relative to the root) itself, by
    /// its path or in an id: one of pytest's "initial paths".
    pub(crate) fn names_file(&self, path: &str) -> bool {
        let file: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        self.readings.iter().any(|reading| match reading {
            Reading::Path(components) => {
                components.len() == file.len() && starts_with(&file, components)
            }
            Reading::Id { path: named, .. } => *named == path,
            Reading::Dotted { .. } | Reading::Key(_) => false,
        })
    }

    /// Whether everything `other` could select, `self` selects too, as
    /// pytest's `is_collection_argument_subsumed_by` decides it for paths and
    /// ids: a file or directory subsumes what lies below it, an id the ids it
    /// prefixes by whole names, and an id without a parameter id every
    /// parametrization. Equal selectors subsume each other. Only a path or an
    /// id with a single reading can be subsumed.
    pub(crate) fn subsumes(&self, other: &Self) -> bool {
        let [only] = other.readings.as_slice() else {
            return false;
        };
        let (path, names, params): (Vec<&str>, &[String], Option<&str>) = match only {
            Reading::Path(components) => {
                (components.iter().map(String::as_str).collect(), &[], None)
            }
            Reading::Id {
                path,
                names,
                params,
            } => (
                path.split('/').filter(|c| !c.is_empty()).collect(),
                names,
                params.as_deref(),
            ),
            Reading::Dotted { .. } | Reading::Key(_) => return false,
        };
        self.readings.iter().any(|reading| match reading {
            Reading::Path(components) => starts_with(&path, components),
            Reading::Id {
                path: by_path,
                names: by_names,
                params: by_params,
            } => {
                let by_path: Vec<&str> = by_path.split('/').filter(|c| !c.is_empty()).collect();
                by_path == path
                    && by_names.len() <= names.len()
                    && by_names.iter().zip(names).all(|(a, b)| a == b)
                    && by_params
                        .as_deref()
                        .is_none_or(|by| Some(by) == params && by_names.len() == names.len())
            }
            Reading::Dotted { .. } | Reading::Key(_) => false,
        })
    }
}

impl Reading {
    /// How specific this reading is; see [`Selector::select_ranked`].
    fn rank(&self) -> usize {
        match self {
            Self::Path(components) => components.len(),
            Self::Id {
                path,
                names,
                params,
            } => {
                path.split('/').filter(|c| !c.is_empty()).count()
                    + names.len()
                    + usize::from(params.is_some())
            }
            Self::Dotted { names, params } => names.len() + usize::from(params.is_some()),
            Self::Key(_) => usize::MAX,
        }
    }
}

impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

enum Found {
    No,
    Whole,
    /// One parameter id of a `Partial` case.
    Narrow(TestId),
}

impl From<bool> for Found {
    fn from(found: bool) -> Self {
        if found { Self::Whole } else { Self::No }
    }
}

/// A case's id, taken apart once for every selector and the keyword matcher.
pub(crate) struct Parts<'a> {
    pub(crate) case: &'a Case,
    /// The file, relative to the project root.
    pub(crate) path: &'a str,
    /// The file's path components, the file name last.
    pub(crate) components: Vec<&'a str>,
    /// The names after the path, without the parameter id.
    pub(crate) names: Vec<&'a str>,
    /// The parameter id, without its brackets.
    pub(crate) params: Option<&'a str>,
    /// The id as dotted names, for Python files: its path as a module, then
    /// its names (`pkg.mod.Class.test`, unittest's), and, when its first
    /// name is itself dotted, its names alone (`pkg.mod.func`, a doctest's,
    /// which names the module as it is imported).
    dotted: Vec<Vec<&'a str>>,
}

impl<'a> Parts<'a> {
    pub(crate) fn new(case: &'a Case) -> Self {
        let id = case.id.as_str();
        let (path, rest) = id.split_once("::").unwrap_or((id, ""));
        let (names, params) = if rest.is_empty() {
            (Vec::new(), None)
        } else {
            let (names, params) = split_params(rest);
            (names.split("::").collect(), params)
        };
        let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        let mut dotted = Vec::new();
        if let Some((file, dirs)) = components.split_last()
            && let Some(module) = file.strip_suffix(".py")
        {
            let mut from_path = dirs.to_vec();
            if module != "__init__" {
                from_path.push(module);
            }
            from_path.extend(&names);
            dotted.push(from_path);
            if names.first().is_some_and(|first| first.contains('.')) {
                dotted.push(names.iter().flat_map(|name| name.split('.')).collect());
            }
        }
        Self {
            case,
            path,
            components,
            names,
            params,
            dotted,
        }
    }

    /// Wanted names against the case's `have` (its names, or its dotted
    /// name). Without a parameter id, wanted names select every case they
    /// prefix; with one, the names must be the case's own.
    fn names(&self, wanted: &[String], params: Option<&str>, have: &[&str]) -> Found {
        let Some(params) = params else {
            return Found::from(starts_with(have, wanted));
        };
        if wanted.len() != have.len() || !starts_with(have, wanted) {
            return Found::No;
        }
        match self.params {
            Some(own) => Found::from(own == params),
            None if self.case.certainty == Certainty::Partial => {
                Found::Narrow(TestId::new(format!("{}[{params}]", self.case.id)))
            }
            None => Found::No,
        }
    }
}

fn starts_with(have: &[&str], prefix: &[String]) -> bool {
    prefix.len() <= have.len() && prefix.iter().zip(have).all(|(p, h)| p == h)
}

/// `test_b[x]` → (`test_b`, `x`). Names never contain `[`, so the first one
/// opens the parameter id, which may itself contain `[`, `::` or `.`.
pub(crate) fn split_params(names: &str) -> (&str, Option<&str>) {
    match names.find('[') {
        Some(at) if names.ends_with(']') => (&names[..at], Some(&names[at + 1..names.len() - 1])),
        _ => (names, None),
    }
}

/// Path components without empty and `.` parts, `..` folded where it can be.
fn normalize(path: &str) -> Vec<String> {
    let mut components: Vec<String> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." if components.last().is_some_and(|last| last != "..") => {
                components.pop();
            }
            other => components.push(other.to_string()),
        }
    }
    components
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(id: &str, certainty: Certainty) -> Case {
        let mut case = Case::new(id, "pytest");
        case.certainty = certainty;
        case
    }

    /// The truth and the narrowed ids of `selector` on the case `id`.
    fn on(selector: &Selector, id: &str, certainty: Certainty) -> (Truth, Vec<String>) {
        let case = case(id, certainty);
        let mut narrow = Vec::new();
        let truth = selector.select(&Parts::new(&case), &mut narrow);
        (truth, narrow.into_iter().map(|id| id.0).collect())
    }

    fn selects(selector: &str, id: &str) -> Truth {
        on(&Selector::parse(selector), id, Certainty::Exact).0
    }

    #[test]
    fn parts_take_an_id_apart() {
        let nested = case(
            "tests/unit/test_x.py::TestA::test_b[a::b.c[d]]",
            Certainty::Exact,
        );
        let parts = Parts::new(&nested);
        assert_eq!(parts.path, "tests/unit/test_x.py");
        assert_eq!(parts.components, ["tests", "unit", "test_x.py"]);
        assert_eq!(parts.names, ["TestA", "test_b"]);
        assert_eq!(parts.params, Some("a::b.c[d]"));
        assert_eq!(
            parts.dotted,
            [["tests", "unit", "test_x", "TestA", "test_b"]]
        );

        let init = case("pkg/__init__.py::test_x", Certainty::Exact);
        assert_eq!(Parts::new(&init).dotted, [["pkg", "test_x"]]);
        let text = case("docs/guide.rst", Certainty::Exact);
        let parts = Parts::new(&text);
        assert!(parts.names.is_empty() && parts.params.is_none() && parts.dotted.is_empty());
        let doctest = case("src/pkg/mod.py::pkg.mod.C.f", Certainty::Exact);
        assert_eq!(
            Parts::new(&doctest).dotted,
            [
                vec!["src", "pkg", "mod", "pkg.mod.C.f"],
                vec!["pkg", "mod", "C", "f"]
            ]
        );
    }

    #[test]
    fn paths_select_by_whole_components() {
        let id = "tests/unit/test_x.py::test_a";
        assert_eq!(selects("tests", id), Truth::True);
        assert_eq!(selects("tests/unit", id), Truth::True);
        assert_eq!(selects("tests/unit/", id), Truth::True);
        assert_eq!(selects("./tests//unit/test_x.py", id), Truth::True);
        assert_eq!(selects("tests/other/../unit", id), Truth::True);
        assert_eq!(selects(".", id), Truth::True);
        assert_eq!(selects("", id), Truth::True);
        assert_eq!(selects("tests/un", id), Truth::False);
        assert_eq!(selects("tests/unit/test_x.py/x", id), Truth::False);
        assert_eq!(selects("unit", id), Truth::False);
        assert_eq!(selects("../tests", id), Truth::False);
        assert_eq!(selects("test_x.py", "test_x.py::test_a"), Truth::True);
        assert_eq!(selects("test_x.p", "test_x.py::test_a"), Truth::False);
    }

    #[test]
    fn ids_select_by_whole_names() {
        let id = "tests/t.py::TestA::test_b";
        assert_eq!(selects("tests/t.py::TestA::test_b", id), Truth::True);
        assert_eq!(selects("tests/t.py::TestA", id), Truth::True);
        assert_eq!(selects("./tests/t.py::TestA", id), Truth::True);
        assert_eq!(selects("tests/t.py::TestA::test", id), Truth::False);
        assert_eq!(selects("tests/t.py::Test", id), Truth::False);
        assert_eq!(selects("tests/t.py::test_b", id), Truth::False);
        assert_eq!(selects("tests/u.py::TestA", id), Truth::False);
        assert_eq!(selects("tests::TestA", id), Truth::False);
        assert_eq!(selects("tests/t.py::", id), Truth::False);
        assert_eq!(selects("tests/t.py::TestA::test_b::x", id), Truth::False);
    }

    #[test]
    fn an_id_without_params_selects_all_of_them() {
        assert_eq!(selects("t.py::test_b", "t.py::test_b[x]"), Truth::True);
        assert_eq!(selects("t.py::test_b", "t.py::test_b[a::b]"), Truth::True);
        assert_eq!(selects("t.py::test", "t.py::test_b[x]"), Truth::False);
        assert_eq!(selects("t.py::test_b[x]", "t.py::test_b[x]"), Truth::True);
        assert_eq!(
            selects("t.py::test_b[a::b]", "t.py::test_b[a::b]"),
            Truth::True
        );
        assert_eq!(selects("t.py::test_b[x]", "t.py::test_b[y]"), Truth::False);
        assert_eq!(selects("t.py::test_b[x]", "t.py::test_b"), Truth::False);
        assert_eq!(
            selects("t.py::TestA[x]", "t.py::TestA::test_b[x]"),
            Truth::False
        );
    }

    #[test]
    fn a_param_id_narrows_a_partial_case() {
        let selector = Selector::parse("t.py::A::test_b[x]");
        assert_eq!(
            on(&selector, "t.py::A::test_b", Certainty::Partial),
            (Truth::Unknown, vec!["t.py::A::test_b[x]".to_string()])
        );
        assert_eq!(
            on(&selector, "t.py::A::test_c", Certainty::Partial).0,
            Truth::False
        );
        assert_eq!(
            on(&selector, "t.py::A::test_b", Certainty::Refined).0,
            Truth::False
        );
        let whole = Selector::parse("t.py::A::test_b");
        assert_eq!(
            on(&whole, "t.py::A::test_b", Certainty::Partial),
            (Truth::True, vec![])
        );
    }

    #[test]
    fn dotted_names_read_from_the_root() {
        let id = "pkg/mod.py::Class::test_a";
        for selector in ["pkg", "pkg.mod", "pkg.mod.Class", "pkg.mod.Class.test_a"] {
            assert_eq!(selects(selector, id), Truth::True, "{selector}");
        }
        for selector in [
            "mod.Class",
            "pkg.mo",
            "pkg.mod.Class.test",
            "pkg.mod.test_a",
        ] {
            assert_eq!(selects(selector, id), Truth::False, "{selector}");
        }
        assert_eq!(
            selects("pkg.test_x", "pkg/__init__.py::test_x"),
            Truth::True
        );
        assert_eq!(selects("test_x", "test_x.py::test_a"), Truth::True);
        assert_eq!(selects("guide", "guide.rst::guide"), Truth::False);
        assert_eq!(selects("m.test_b", "m.py::test_b[1.5]"), Truth::True);
        assert_eq!(selects("m.test_b[1.5]", "m.py::test_b[1.5]"), Truth::True);
        assert_eq!(selects("m.test_b[2]", "m.py::test_b[1.5]"), Truth::False);
        assert_eq!(
            on(
                &Selector::parse("m.test_b[1.5]"),
                "m.py::test_b",
                Certainty::Partial
            ),
            (Truth::Unknown, vec!["m.py::test_b[1.5]".to_string()])
        );
    }

    /// A doctest's id names its object by the module's import name
    /// (`pkg/mod.py::pkg.mod.func`): its dotted name selects it, as
    /// unittest's `pkg.mod.Class.test` selects a `TestCase` method.
    #[test]
    fn dotted_names_select_doctests_by_their_own_names() {
        let id = "pkg/mod.py::pkg.mod.Class.method";
        for selector in ["pkg", "pkg.mod", "pkg.mod.Class", "pkg.mod.Class.method"] {
            assert_eq!(selects(selector, id), Truth::True, "{selector}");
        }
        for selector in ["pkg.mod.Cla", "mod.Class.method", "pkg.mod.Class.method.x"] {
            assert_eq!(selects(selector, id), Truth::False, "{selector}");
        }
        // The module's own docstring, and a package's.
        assert_eq!(selects("pkg.mod", "pkg/mod.py::pkg.mod"), Truth::True);
        assert_eq!(selects("pkg", "pkg/__init__.py::pkg"), Truth::True);
        // Under a `src/` layout, the import name is not the path.
        assert_eq!(
            selects("pkg.mod.func", "src/pkg/mod.py::pkg.mod.func"),
            Truth::True
        );
        assert_eq!(
            selects("src.pkg.mod", "src/pkg/mod.py::pkg.mod.func"),
            Truth::True
        );
        // A text file's doctest is selected by its path only.
        assert_eq!(
            selects("test_usage.txt", "docs/test_usage.txt::test_usage.txt"),
            Truth::False
        );
        // Names that aren't dotted are read from the path alone.
        assert_eq!(
            selects("Class.test", "pkg/mod.py::Class::test"),
            Truth::False
        );
    }

    #[test]
    fn words_that_are_not_dotted_names_are_only_paths() {
        let selector = Selector::parse("my-tests");
        assert_eq!(selector.readings, [Reading::Path(vec!["my-tests".into()])]);
        assert_eq!(selects("my-tests", "my-tests/t.py::test_a"), Truth::True);
        assert_eq!(Selector::parse("a..b").readings.len(), 1);
        assert_eq!(Selector::parse("1abc.x").readings.len(), 1);
    }

    #[test]
    fn keys_select_by_hex_prefix() {
        let id = "tests/t.py::test_a[1]";
        let key = TestId::new(id).key().0;
        assert_eq!(selects(&key, id), Truth::True);
        assert_eq!(selects(&key[..4], id), Truth::True);
        assert_eq!(selects(&key[..6].to_ascii_uppercase(), id), Truth::True);
        assert_eq!(
            selects(&key[..3], id),
            Truth::False,
            "too short to be a key"
        );
        let other = if key.starts_with('0') { "1111" } else { "0000" };
        assert_eq!(selects(other, id), Truth::False);
        assert!(
            !Selector::parse("0123456789abcdef0")
                .readings
                .iter()
                .any(|r| matches!(r, Reading::Key(_))),
            "longer than a key"
        );
    }

    #[test]
    fn hex_words_are_also_paths() {
        assert_eq!(selects("cafe", "cafe/t.py::test_a"), Truth::True);
        assert_eq!(selects("cafe", "cafe.py::test_a"), Truth::True);
    }

    #[test]
    fn explicit_paths_are_only_paths() {
        let selector = Selector::path("pkg.mod");
        assert_eq!(
            on(&selector, "pkg/mod.py::test_a", Certainty::Exact).0,
            Truth::False
        );
        assert_eq!(
            on(&selector, "pkg.mod/t.py::test_a", Certainty::Exact).0,
            Truth::True
        );
        assert_eq!(selector.as_str(), "pkg.mod");
        assert_eq!(selector.to_string(), "pkg.mod");
    }
}
