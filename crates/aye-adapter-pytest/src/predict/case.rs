//! An item, as pytest's `Function` holds it, made a case: its id, span,
//! tags, keywords, timeout and affinity.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **Tags** are mark names in pytest's `iter_markers` order (the function's
//!   decorators, its parameter set's marks, then each class's, innermost
//!   first, then the module's), deduplicated. **Keywords** are the names
//!   pytest's `-k` matches: directories below the rootdir, the file name,
//!   class names, the test name with and without parameters, the names in
//!   the function's `__dict__` (`pytestmark` after mark decorators,
//!   `__wrapped__` and `patchings` from other decorators, attributes the
//!   module sets), and the tags. `timeout` comes from the closest
//!   `@pytest.mark.timeout`, as pytest-timeout reads it; no other mark maps
//!   onto case metadata.

use aye_adapter::TextRange;
use aye_events::{AffinityKey, Case, Certainty};

use super::ModuleRecognizer;
use crate::marks::{self, Mark, SetTimeout};

/// The parts of a test's identity and metadata, before it becomes a case.
pub(super) struct Item<'s, 'a> {
    pub(super) classes: Vec<String>,
    pub(super) name: String,
    pub(super) originalname: String,
    pub(super) span: TextRange,
    /// In pytest's `iter_markers` order: the function's, the parameter
    /// set's, each class's (innermost first), the module's.
    pub(super) marks: Vec<Mark<'s, 'a>>,
    /// How many of `marks` are the function's own.
    pub(super) fn_marks: usize,
    /// Marks from `pytest.param(marks=…)` and the empty parameter set.
    pub(super) param_marks: Vec<String>,
    /// The timeout the first `timeout` mark among `param_marks` sets.
    pub(super) param_timeout: SetTimeout,
    /// Names in the function's `__dict__`: what decorators leave there,
    /// and attributes the module sets.
    pub(super) attributes: Vec<String>,
    pub(super) certainty: Certainty,
}

impl<'s, 'a> ModuleRecognizer<'s, 'a> {
    pub(super) fn push(&mut self, item: &Item<'s, 'a>) {
        self.keys.push(Vec::new());
        let mut case = Case::new(
            format!("{}::{}", self.nodeid(&item.classes), item.name),
            crate::NAME,
        );
        case.span = Some(self.file.span(item.span));
        // `iter_markers` order: the function's own marks (its decorators,
        // then the parameter set's), the classes', the module's.
        let names = item.marks[..item.fn_marks]
            .iter()
            .map(|m| m.name.clone())
            .chain(item.param_marks.iter().cloned())
            .chain(item.marks[item.fn_marks..].iter().map(|m| m.name.clone()));
        let mut tags: Vec<String> = Vec::new();
        for name in names {
            if !tags.contains(&name) {
                tags.push(name);
            }
        }
        // pytest-timeout reads the closest `timeout` mark.
        let own = item.marks[..item.fn_marks]
            .iter()
            .find(|m| m.name == "timeout");
        let rest = || {
            item.marks[item.fn_marks..]
                .iter()
                .find(|m| m.name == "timeout")
        };
        case.timeout = match (own, item.param_timeout) {
            (Some(mark), _) => marks::timeout(mark),
            (None, SetTimeout::Seconds(seconds)) => Some(seconds),
            (None, SetTimeout::Unreadable) => None,
            (None, SetTimeout::Unmarked) => rest().and_then(marks::timeout),
        };
        let mut keywords: Vec<String> = Vec::new();
        let mut add = |k: &str| {
            if !keywords.iter().any(|x| x == k) {
                keywords.push(k.to_string());
            }
        };
        for k in self.path_keywords {
            add(k);
        }
        for class in &item.classes {
            add(class);
        }
        add(&item.originalname);
        add(&item.name);
        // Mark decorators store `pytestmark` in the function's `__dict__`.
        if item.fn_marks > 0 {
            add("pytestmark");
        }
        for attribute in &item.attributes {
            add(attribute);
        }
        for tag in &tags {
            add(tag);
        }
        case.keywords = keywords;
        case.tags = tags;
        case.affinity.push(AffinityKey::file(self.file.rel));
        for i in 1..=item.classes.len() {
            case.affinity
                .push(AffinityKey::file_class(self.file.rel, &item.classes[..i]));
        }
        case.certainty = item.certainty;
        self.cases.push(case);
    }
}
