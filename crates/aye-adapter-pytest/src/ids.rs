//! pytest's parameter id algorithm (`_pytest/python.py`, `IdMaker`), and the
//! Python `str()` of the literal values it turns into ids: pytest's
//! `ascii_escaped` here, the `repr` of numbers and `str.isdigit` from
//! aye-python-stdlib.

use std::collections::HashMap;
use std::fmt::Write as _;

use aye_python_stdlib::builtins::str_isdigit;
use aye_python_stdlib::repr::{complex_repr, float_repr};

/// A statically known parameter value, as far as ids are concerned.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Str(String),
    Bytes(Vec<u8>),
    /// Decimal digits, with a leading `-` when negative.
    Int(String),
    Float(f64),
    Complex(f64, f64),
    Bool(bool),
    None,
    /// An object identified by its `__name__`: a function, a class,
    /// `<lambda>`.
    Named(String),
    /// A compiled regular expression; its id is its escaped pattern.
    Pattern(Box<Value>),
    /// A value with no id of its own (list, dict, tuple…): the id falls back
    /// to the argument name and the parameter set's index.
    Opaque,
    /// Not known statically. Harmless inside a parameter set whose id is
    /// explicit, but a lone unknown value may itself be a `pytest.param`
    /// (a helper's result) whose id and marks win over `ids=`.
    Unknown,
}

/// One parameter set (`pytest.param(...)` or a plain value).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParamSet {
    pub values: Vec<Value>,
    /// `pytest.param(..., id=...)`.
    pub id: Option<IdItem>,
    /// Mark names from `pytest.param(..., marks=...)`, plus the empty
    /// parameter set mark.
    pub marks: Vec<String>,
}

/// An explicit id: a value to turn into an id, or `pytest.HIDDEN_PARAM`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum IdItem {
    /// From `pytest.param(id=...)`: a string used as is (escaped).
    Text(String),
    /// From an `ids=[...]` list: any supported value.
    Value(Value),
    Hidden,
}

/// Settings that change ids.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct IdOptions {
    /// `disable_test_id_escaping_and_forfeit_all_rights_to_community_support`.
    pub no_escaping: bool,
    /// `strict_parametrization_ids` (or `strict`).
    pub strict: bool,
}

/// Why pytest would refuse a parametrization (a collection error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IdError {
    /// An id depends on a value only known at runtime.
    UnknownValue(usize),
    Duplicates(Vec<String>),
    MultipleHidden,
    UnsupportedId(usize),
    BytesWithoutEscaping,
    /// A duplicated id ends in a character whose `str.isdigit` depends on
    /// the Python version (its Unicode database).
    VersionDependentDigit(String),
}

impl std::fmt::Display for IdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownValue(at) => write!(f, "parameter set {at} is not a literal"),
            Self::Duplicates(ids) => write!(
                f,
                "duplicate parametrization ids under strict_parametrization_ids: {}",
                ids.join(", ")
            ),
            Self::MultipleHidden => f.write_str("more than one HIDDEN_PARAM in one parametrize"),
            Self::UnsupportedId(at) => write!(f, "ids contains an unsupported value at index {at}"),
            Self::BytesWithoutEscaping => {
                f.write_str("bytes parameters with test id escaping disabled")
            }
            Self::VersionDependentDigit(id) => write!(
                f,
                "whether the duplicate id {id:?} ends in a digit depends on the Python version"
            ),
        }
    }
}

/// `make_unique_parameterset_ids`: one id per parameter set, `None` for a
/// hidden one.
pub(crate) fn parameterset_ids(
    argnames: &[String],
    sets: &[ParamSet],
    ids: Option<&[Option<IdItem>]>,
    options: IdOptions,
) -> Result<Vec<Option<String>>, IdError> {
    let mut resolved = Vec::with_capacity(sets.len());
    for (index, set) in sets.iter().enumerate() {
        // `_resolve_ids` takes a `ParameterSet`'s own id before `ids=`: a lone
        // value aye can't read may be one (`make(1)` returning
        // `pytest.param(1, id="z1", marks=…)`).
        if set.id.is_none() && set.marks.is_empty() && set.values == [Value::Unknown] {
            return Err(IdError::UnknownValue(index));
        }
        let explicit = set.id.as_ref().or_else(|| {
            ids.filter(|ids| !ids.is_empty())
                .and_then(|ids| ids.get(index))
                .and_then(Option::as_ref)
        });
        resolved.push(match explicit {
            Some(IdItem::Hidden) => None,
            Some(IdItem::Text(text)) => {
                Some(escaped_by_config(&Value::Str(text.clone()), options)?)
            }
            Some(IdItem::Value(Value::Unknown)) => return Err(IdError::UnknownValue(index)),
            Some(IdItem::Value(value)) => {
                Some(idval_from_value(value, options)?.ok_or(IdError::UnsupportedId(index))?)
            }
            None => {
                let mut parts = Vec::with_capacity(argnames.len());
                for (value, argname) in set.values.iter().zip(argnames) {
                    if *value == Value::Unknown {
                        return Err(IdError::UnknownValue(index));
                    }
                    parts.push(match idval_from_value(value, options)? {
                        Some(id) => id,
                        None => format!("{argname}{index}"),
                    });
                }
                Some(parts.join("-"))
            }
        });
    }

    let mut counts: HashMap<Option<String>, usize> = HashMap::new();
    for id in &resolved {
        *counts.entry(id.clone()).or_default() += 1;
    }
    if counts.len() == resolved.len() {
        return Ok(resolved);
    }
    if options.strict {
        let mut duplicates: Vec<String> = Vec::new();
        for id in &resolved {
            let shown = id.clone().unwrap_or_else(|| "<hidden>".to_string());
            if counts[id] > 1 && !duplicates.contains(&shown) {
                duplicates.push(shown);
            }
        }
        return Err(IdError::Duplicates(duplicates));
    }
    if counts.get(&None).is_some_and(|n| *n > 1) {
        return Err(IdError::MultipleHidden);
    }

    let original_counts = counts.clone();
    let mut suffixes: HashMap<String, usize> = HashMap::new();
    for slot in &mut resolved {
        let Some(id) = slot.clone() else {
            continue;
        };
        if original_counts[&Some(id.clone())] <= 1 {
            continue;
        }
        // Non-ASCII ids come from `__name__`s, which are never escaped, and
        // from strings when escaping is disabled.
        let separator = match id.chars().last().map_or(Some(false), str_isdigit) {
            Some(true) => "_",
            Some(false) => "",
            None => return Err(IdError::VersionDependentDigit(id)),
        };
        let next = suffixes.entry(id.clone()).or_default();
        let mut candidate = format!("{id}{separator}{next}");
        while counts.get(&Some(candidate.clone())).is_some_and(|n| *n > 0) {
            *next += 1;
            candidate = format!("{id}{separator}{next}");
        }
        *next += 1;
        if let Some(n) = counts.get_mut(&Some(id)) {
            *n -= 1;
        }
        *counts.entry(Some(candidate.clone())).or_default() += 1;
        *slot = Some(candidate);
    }
    Ok(resolved)
}

/// `_idval_from_value`: the id a value makes on its own, if it makes one.
pub(crate) fn idval_from_value(
    value: &Value,
    options: IdOptions,
) -> Result<Option<String>, IdError> {
    Ok(match value {
        Value::Str(_) | Value::Bytes(_) => Some(escaped_by_config(value, options)?),
        Value::None => Some("None".to_string()),
        Value::Bool(b) => Some(if *b { "True" } else { "False" }.to_string()),
        Value::Int(digits) => Some(digits.clone()),
        Value::Float(f) => Some(float_repr(*f)),
        Value::Complex(re, im) => Some(complex_repr(*re, *im)),
        Value::Pattern(pattern) => match pattern.as_ref() {
            Value::Str(s) => Some(ascii_escaped_str(s)),
            Value::Bytes(b) => Some(ascii_escaped_bytes(b)),
            _ => None,
        },
        Value::Named(name) => Some(name.clone()),
        Value::Opaque | Value::Unknown => None,
    })
}

fn escaped_by_config(value: &Value, options: IdOptions) -> Result<String, IdError> {
    match value {
        Value::Str(s) if options.no_escaping => Ok(s.clone()),
        Value::Str(s) => Ok(ascii_escaped_str(s)),
        Value::Bytes(_) if options.no_escaping => Err(IdError::BytesWithoutEscaping),
        Value::Bytes(b) => Ok(ascii_escaped_bytes(b)),
        _ => unreachable!("only strings and bytes are escaped"),
    }
}

/// `ascii_escaped(str)`: `str.encode("unicode_escape")`, then the
/// non-printable translation table (which has nothing left to do).
pub(crate) fn ascii_escaped_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let code = u32::from(c);
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            ' '..='~' => out.push(c),
            _ if code < 0x100 => {
                let _ = write!(out, "\\x{code:02x}");
            }
            _ if code < 0x1_0000 => {
                let _ = write!(out, "\\u{code:04x}");
            }
            _ => {
                let _ = write!(out, "\\U{code:08x}");
            }
        }
    }
    out
}

/// `ascii_escaped(bytes)`: `bytes.decode("ascii", "backslashreplace")`,
/// then the non-printable translation table.
pub(crate) fn ascii_escaped_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b' '..=b'~' => out.push(char::from(b)),
            _ => {
                let _ = write!(out, "\\x{b:02x}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(argnames: &[&str], sets: Vec<Vec<Value>>) -> Vec<String> {
        let argnames: Vec<String> = argnames.iter().map(|s| (*s).to_string()).collect();
        let sets: Vec<ParamSet> = sets
            .into_iter()
            .map(|values| ParamSet {
                values,
                id: None,
                marks: Vec::new(),
            })
            .collect();
        parameterset_ids(&argnames, &sets, None, IdOptions::default())
            .unwrap()
            .into_iter()
            .map(Option::unwrap)
            .collect()
    }

    fn one(value: Value) -> String {
        ids(&["v"], vec![vec![value]]).remove(0)
    }

    fn s(text: &str) -> Value {
        Value::Str(text.to_string())
    }

    fn int(text: &str) -> Value {
        Value::Int(text.to_string())
    }

    #[test]
    fn strings_and_bytes_are_ascii_escaped() {
        // Expected values from `pytest --collect-only` on pytest 9.1.1.
        assert_eq!(one(s("é")), "\\xe9");
        assert_eq!(one(s("日本")), "\\u65e5\\u672c");
        assert_eq!(one(s("\t\n\r")), "\\t\\n\\r");
        assert_eq!(one(s("a\\b")), "a\\\\b");
        assert_eq!(one(s("\u{0}\u{7f}")), "\\x00\\x7f");
        assert_eq!(one(s("😀")), "\\U0001f600");
        assert_eq!(one(s("")), "");
        assert_eq!(one(s("x::y")), "x::y");
        assert_eq!(one(s("[x]")), "[x]");
        assert_eq!(one(s("\u{80}")), "\\x80");
        assert_eq!(one(s("\u{ffff}")), "\\uffff");
        assert_eq!(one(s("ß")), "\\xdf");
        assert_eq!(one(Value::Bytes(b"abc".to_vec())), "abc");
        assert_eq!(one(Value::Bytes(vec![0xc3, 0xa9])), "\\xc3\\xa9");
        assert_eq!(one(Value::Bytes(b"\t\n".to_vec())), "\\t\\n");
        assert_eq!(one(Value::Bytes(b"\\".to_vec())), "\\");
        assert_eq!(one(Value::Bytes(vec![0, 0x7f])), "\\x00\\x7f");
    }

    #[test]
    fn other_values() {
        assert_eq!(one(Value::None), "None");
        assert_eq!(one(Value::Bool(true)), "True");
        assert_eq!(one(int("-1")), "-1");
        assert_eq!(one(Value::Complex(1.0, 2.0)), "(1+2j)");
        assert_eq!(one(Value::Opaque), "v0");
        assert_eq!(one(Value::Named("int".into())), "int");
        assert_eq!(one(Value::Pattern(Box::new(s("a.é")))), "a.\\xe9");
    }

    #[test]
    fn duplicates_get_suffixes_like_pytest() {
        // Expected values from `pytest --collect-only` on pytest 9.1.1.
        assert_eq!(
            ids(
                &["x"],
                vec![
                    vec![int("1")],
                    vec![int("1")],
                    vec![int("2")],
                    vec![int("1")]
                ]
            ),
            ["1_0", "1_1", "2", "1_2"]
        );
        assert_eq!(
            ids(
                &["x"],
                vec![
                    vec![s("a")],
                    vec![s("a")],
                    vec![s("a1")],
                    vec![s("a1")],
                    vec![s("b")]
                ]
            ),
            ["a0", "a2", "a1_0", "a1_1", "b"]
        );
        assert_eq!(
            ids(&["x"], vec![vec![s("a0")], vec![s("a")], vec![s("a")]]),
            ["a0", "a1", "a2"]
        );
        assert_eq!(ids(&["x"], vec![vec![s("")], vec![s("")]]), ["0", "1"]);
    }

    #[test]
    fn explicit_ids_and_hidden_params() {
        let argnames = vec!["x".to_string()];
        let sets: Vec<ParamSet> = (1..=3)
            .map(|n| ParamSet {
                values: vec![int(&n.to_string())],
                id: None,
                marks: Vec::new(),
            })
            .collect();
        let given = [
            Some(IdItem::Value(s("one"))),
            None,
            Some(IdItem::Value(Value::Float(1.5))),
        ];
        assert_eq!(
            parameterset_ids(&argnames, &sets, Some(&given), IdOptions::default()).unwrap(),
            [Some("one".into()), Some("2".into()), Some("1.5".into())]
        );
        let unsupported = [None, Some(IdItem::Value(Value::Opaque)), None];
        assert_eq!(
            parameterset_ids(&argnames, &sets, Some(&unsupported), IdOptions::default()),
            Err(IdError::UnsupportedId(1))
        );
        let hidden = [Some(IdItem::Hidden), Some(IdItem::Hidden), None];
        assert_eq!(
            parameterset_ids(&argnames, &sets, Some(&hidden), IdOptions::default()),
            Err(IdError::MultipleHidden)
        );
        let same = [
            Some(IdItem::Value(s("same"))),
            Some(IdItem::Value(s("same"))),
            None,
        ];
        assert_eq!(
            parameterset_ids(&argnames, &sets, Some(&same), IdOptions::default()).unwrap(),
            [Some("same0".into()), Some("same1".into()), Some("3".into())]
        );
        let strict = IdOptions {
            strict: true,
            ..IdOptions::default()
        };
        assert!(matches!(
            parameterset_ids(&argnames, &sets, Some(&same), strict),
            Err(IdError::Duplicates(_))
        ));
        let no_escaping = IdOptions {
            no_escaping: true,
            ..IdOptions::default()
        };
        let text = [Some(IdItem::Text("é".into())), None, None];
        assert_eq!(
            parameterset_ids(&argnames, &sets, Some(&text), no_escaping).unwrap()[0],
            Some("é".into())
        );
    }

    #[test]
    fn a_lone_unknown_value_may_be_a_parameter_set() {
        // `make(1)` may return `pytest.param(1, id="z1", marks=…)`, whose id
        // and marks win over `ids=`: pytest gives test_hp[z1], not [a].
        let argnames = vec!["x".to_string()];
        let set = |values: Vec<Value>, id: Option<IdItem>, marks: Vec<String>| ParamSet {
            values,
            id,
            marks,
        };
        let given = [Some(IdItem::Value(s("a")))];
        let lone = [set(vec![Value::Unknown], None, Vec::new())];
        assert_eq!(
            parameterset_ids(&argnames, &lone, Some(&given), IdOptions::default()),
            Err(IdError::UnknownValue(0))
        );
        assert_eq!(
            parameterset_ids(&argnames, &lone, None, IdOptions::default()),
            Err(IdError::UnknownValue(0))
        );
        // Inside a `pytest.param` (with its own id or marks), or a tuple of
        // several values, an unknown value is only a value.
        let param = [set(
            vec![Value::Unknown],
            Some(IdItem::Text("p".into())),
            Vec::new(),
        )];
        assert_eq!(
            parameterset_ids(&argnames, &param, Some(&given), IdOptions::default()).unwrap(),
            [Some("p".into())]
        );
        let marked = [set(vec![Value::Unknown], None, vec!["slow".into()])];
        assert_eq!(
            parameterset_ids(&argnames, &marked, Some(&given), IdOptions::default()).unwrap(),
            [Some("a".into())]
        );
        let two = vec!["x".to_string(), "y".to_string()];
        let row = [set(vec![Value::Unknown, int("1")], None, Vec::new())];
        assert_eq!(
            parameterset_ids(&two, &row, Some(&given), IdOptions::default()).unwrap(),
            [Some("a".into())]
        );
    }

    #[test]
    fn duplicate_suffixes_follow_str_isdigit() {
        // pytest 9.1.1 with escaping disabled: a½0, a½1, a〇0, a〇1, Ⅻ0,
        // Ⅻ1, x²_0, x²_1.
        let no_escaping = IdOptions {
            no_escaping: true,
            ..IdOptions::default()
        };
        let argnames = vec!["s".to_string()];
        let sets: Vec<ParamSet> = ["a½", "a½", "a〇", "a〇", "Ⅻ", "Ⅻ", "x²", "x²"]
            .into_iter()
            .map(|text| ParamSet {
                values: vec![s(text)],
                id: None,
                marks: Vec::new(),
            })
            .collect();
        let found: Vec<String> = parameterset_ids(&argnames, &sets, None, no_escaping)
            .unwrap()
            .into_iter()
            .map(Option::unwrap)
            .collect();
        assert_eq!(
            found,
            ["a½0", "a½1", "a〇0", "a〇1", "Ⅻ0", "Ⅻ1", "x²_0", "x²_1"]
        );
        // `__name__`s are never escaped: f〇0 and f〇1 with escaping on.
        assert_eq!(
            ids(
                &["v"],
                vec![
                    vec![Value::Named("f〇".into())],
                    vec![Value::Named("f〇".into())]
                ]
            ),
            ["f〇0", "f〇1"]
        );
        let tangsa = vec![
            ParamSet {
                values: vec![s("t\u{16ac0}")],
                id: None,
                marks: Vec::new(),
            };
            2
        ];
        assert!(matches!(
            parameterset_ids(&argnames, &tangsa, None, no_escaping),
            Err(IdError::VersionDependentDigit(_))
        ));
    }
}
