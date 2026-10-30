//! pytest's parameter id algorithm (`_pytest/python.py`, `IdMaker`), and the
//! Python `str()` of the literal values it turns into ids.

use std::collections::HashMap;
use std::fmt::Write as _;

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
        let separator = match id.chars().last().map_or(Some(false), is_digit) {
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

/// Python's `str.isdigit` (Unicode `Numeric_Type` `Digit` or `Decimal`),
/// on every Python pytest 9.1.1 supports: 3.10 to 3.14, whose Unicode
/// databases go from 13.0 to 16.0. `None` when they disagree, or for a
/// numeric character the tables below don't cover (Rust's Unicode is
/// newer). Non-ASCII ids come from `__name__`s, which are never escaped,
/// and from strings when escaping is disabled.
fn is_digit(c: char) -> Option<bool> {
    let within = |table: &[(u32, u32)]| {
        let code = u32::from(c);
        table
            .binary_search_by(|&(lo, hi)| {
                if hi < code {
                    std::cmp::Ordering::Less
                } else if lo > code {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    };
    if c.is_ascii() {
        Some(c.is_ascii_digit())
    } else if within(DIGITS) {
        Some(true)
    } else if !c.is_numeric() || within(NUMERIC_NOT_DIGITS) {
        Some(false)
    } else {
        None
    }
}

// Generated from `chr(c).isdigit()` and `chr(c).isnumeric()` on CPython
// 3.10 to 3.14, keeping what every version agrees on (and, for the second
// table, only the general categories Rust's `char::is_numeric` covers).

/// Non-ASCII characters `str.isdigit` accepts on Python 3.10 to 3.14.
#[rustfmt::skip]
const DIGITS: &[(u32, u32)] = &[
    (0x00b2, 0x00b3), (0x00b9, 0x00b9), (0x0660, 0x0669), (0x06f0, 0x06f9), (0x07c0, 0x07c9),
    (0x0966, 0x096f), (0x09e6, 0x09ef), (0x0a66, 0x0a6f), (0x0ae6, 0x0aef), (0x0b66, 0x0b6f),
    (0x0be6, 0x0bef), (0x0c66, 0x0c6f), (0x0ce6, 0x0cef), (0x0d66, 0x0d6f), (0x0de6, 0x0def),
    (0x0e50, 0x0e59), (0x0ed0, 0x0ed9), (0x0f20, 0x0f29), (0x1040, 0x1049), (0x1090, 0x1099),
    (0x1369, 0x1371), (0x17e0, 0x17e9), (0x1810, 0x1819), (0x1946, 0x194f), (0x19d0, 0x19da),
    (0x1a80, 0x1a89), (0x1a90, 0x1a99), (0x1b50, 0x1b59), (0x1bb0, 0x1bb9), (0x1c40, 0x1c49),
    (0x1c50, 0x1c59), (0x2070, 0x2070), (0x2074, 0x2079), (0x2080, 0x2089), (0x2460, 0x2468),
    (0x2474, 0x247c), (0x2488, 0x2490), (0x24ea, 0x24ea), (0x24f5, 0x24fd), (0x24ff, 0x24ff),
    (0x2776, 0x277e), (0x2780, 0x2788), (0x278a, 0x2792), (0xa620, 0xa629), (0xa8d0, 0xa8d9),
    (0xa900, 0xa909), (0xa9d0, 0xa9d9), (0xa9f0, 0xa9f9), (0xaa50, 0xaa59), (0xabf0, 0xabf9),
    (0xff10, 0xff19), (0x104a0, 0x104a9), (0x10a40, 0x10a43), (0x10d30, 0x10d39),
    (0x10e60, 0x10e68), (0x11052, 0x1105a), (0x11066, 0x1106f), (0x110f0, 0x110f9),
    (0x11136, 0x1113f), (0x111d0, 0x111d9), (0x112f0, 0x112f9), (0x11450, 0x11459),
    (0x114d0, 0x114d9), (0x11650, 0x11659), (0x116c0, 0x116c9), (0x11730, 0x11739),
    (0x118e0, 0x118e9), (0x11950, 0x11959), (0x11c50, 0x11c59), (0x11d50, 0x11d59),
    (0x11da0, 0x11da9), (0x16a60, 0x16a69), (0x16b50, 0x16b59), (0x1d7ce, 0x1d7ff),
    (0x1e140, 0x1e149), (0x1e2f0, 0x1e2f9), (0x1e950, 0x1e959), (0x1f100, 0x1f10a),
    (0x1fbf0, 0x1fbf9),
];

/// Non-ASCII characters of the numeric general categories (`Nd`, `Nl`,
/// `No`) `str.isdigit` rejects on Python 3.10 to 3.14.
#[rustfmt::skip]
const NUMERIC_NOT_DIGITS: &[(u32, u32)] = &[
    (0x00bc, 0x00be), (0x09f4, 0x09f9), (0x0b72, 0x0b77), (0x0bf0, 0x0bf2), (0x0c78, 0x0c7e),
    (0x0d58, 0x0d5e), (0x0d70, 0x0d78), (0x0f2a, 0x0f33), (0x1372, 0x137c), (0x16ee, 0x16f0),
    (0x17f0, 0x17f9), (0x2150, 0x2182), (0x2185, 0x2189), (0x2469, 0x2473), (0x247d, 0x2487),
    (0x2491, 0x249b), (0x24eb, 0x24f4), (0x24fe, 0x24fe), (0x277f, 0x277f), (0x2789, 0x2789),
    (0x2793, 0x2793), (0x2cfd, 0x2cfd), (0x3007, 0x3007), (0x3021, 0x3029), (0x3038, 0x303a),
    (0x3192, 0x3195), (0x3220, 0x3229), (0x3248, 0x324f), (0x3251, 0x325f), (0x3280, 0x3289),
    (0x32b1, 0x32bf), (0xa6e6, 0xa6ef), (0xa830, 0xa835), (0x10107, 0x10133), (0x10140, 0x10178),
    (0x1018a, 0x1018b), (0x102e1, 0x102fb), (0x10320, 0x10323), (0x10341, 0x10341),
    (0x1034a, 0x1034a), (0x103d1, 0x103d5), (0x10858, 0x1085f), (0x10879, 0x1087f),
    (0x108a7, 0x108af), (0x108fb, 0x108ff), (0x10916, 0x1091b), (0x109bc, 0x109bd),
    (0x109c0, 0x109cf), (0x109d2, 0x109ff), (0x10a44, 0x10a48), (0x10a7d, 0x10a7e),
    (0x10a9d, 0x10a9f), (0x10aeb, 0x10aef), (0x10b58, 0x10b5f), (0x10b78, 0x10b7f),
    (0x10ba9, 0x10baf), (0x10cfa, 0x10cff), (0x10e69, 0x10e7e), (0x10f1d, 0x10f26),
    (0x10f51, 0x10f54), (0x10fc5, 0x10fcb), (0x1105b, 0x11065), (0x111e1, 0x111f4),
    (0x1173a, 0x1173b), (0x118ea, 0x118f2), (0x11c5a, 0x11c6c), (0x11fc0, 0x11fd4),
    (0x12400, 0x1246e), (0x16b5b, 0x16b61), (0x16e80, 0x16e96), (0x1d2e0, 0x1d2f3),
    (0x1d360, 0x1d378), (0x1e8c7, 0x1e8cf), (0x1ec71, 0x1ecab), (0x1ecad, 0x1ecaf),
    (0x1ecb1, 0x1ecb4), (0x1ed01, 0x1ed2d), (0x1ed2f, 0x1ed3d), (0x1f10b, 0x1f10c),
];

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

/// `repr(float)`.
pub(crate) fn float_repr(f: f64) -> String {
    format_float(f, true, false)
}

/// `repr(complex)`.
pub(crate) fn complex_repr(re: f64, im: f64) -> String {
    if re == 0.0 && re.is_sign_positive() {
        format!("{}j", format_float(im, false, false))
    } else {
        format!(
            "({}{}j)",
            format_float(re, false, false),
            format_float(im, false, true)
        )
    }
}

/// Python's `format_float_short` in `'r'` mode.
fn format_float(f: f64, add_dot_0: bool, always_sign: bool) -> String {
    let sign = if f.is_sign_negative() && !f.is_nan() {
        "-"
    } else if always_sign {
        "+"
    } else {
        ""
    };
    if f.is_nan() {
        return format!("{sign}nan");
    }
    if f.is_infinite() {
        return format!("{sign}inf");
    }
    // Rust's `{:e}` gives the shortest round-trip digits, like dtoa mode 0.
    let sci = format!("{:e}", f.abs());
    let (mantissa, exponent) = sci.split_once('e').expect("exponent");
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exponent: i64 = exponent.parse().expect("integer exponent");
    let decpt = exponent + 1;
    let len = i64::try_from(digits.len()).expect("short");

    let mut out = String::from(sign);
    if decpt <= -4 || decpt > 16 {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let exp = decpt - 1;
        let _ = write!(out, "e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs());
        return out;
    }
    if decpt <= 0 {
        out.push_str("0.");
        out.push_str(&"0".repeat(usize::try_from(-decpt).expect("small")));
        out.push_str(&digits);
    } else if decpt < len {
        let at = usize::try_from(decpt).expect("small");
        out.push_str(&digits[..at]);
        out.push('.');
        out.push_str(&digits[at..]);
    } else {
        out.push_str(&digits);
        out.push_str(&"0".repeat(usize::try_from(decpt - len).expect("small")));
        if add_dot_0 {
            out.push_str(".0");
        }
    }
    out
}

/// The decimal spelling of a Python integer literal (`0x_ff`, `1_000`).
pub(crate) fn int_literal_decimal(token: &str) -> Option<String> {
    let token: String = token.chars().filter(|c| *c != '_').collect();
    let lower = token.to_ascii_lowercase();
    let (radix, digits) = if let Some(rest) = lower.strip_prefix("0x") {
        (16, rest)
    } else if let Some(rest) = lower.strip_prefix("0o") {
        (8, rest)
    } else if let Some(rest) = lower.strip_prefix("0b") {
        (2, rest)
    } else {
        (10, lower.as_str())
    };
    if digits.is_empty() {
        return None;
    }
    // Little-endian limbs of 10^9.
    let mut limbs: Vec<u64> = vec![0];
    for c in digits.chars() {
        let digit = u64::from(c.to_digit(radix)?);
        let mut carry = digit;
        for limb in &mut limbs {
            let value = *limb * u64::from(radix) + carry;
            *limb = value % 1_000_000_000;
            carry = value / 1_000_000_000;
        }
        while carry > 0 {
            limbs.push(carry % 1_000_000_000);
            carry /= 1_000_000_000;
        }
    }
    let mut out = limbs.last().expect("one limb").to_string();
    for limb in limbs.iter().rev().skip(1) {
        let _ = write!(out, "{limb:09}");
    }
    Some(out)
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
    fn floats_print_like_python_repr() {
        // Expected values from `pytest --collect-only` on pytest 9.1.1.
        for (value, expected) in [
            (1.0, "1.0"),
            (1e16, "1e+16"),
            (1e-5, "1e-05"),
            (0.1, "0.1"),
            (1e22, "1e+22"),
            (123_456_789.0, "123456789.0"),
            (1.5e-7, "1.5e-07"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "nan"),
            (-0.0, "-0.0"),
            (0.0, "0.0"),
            (2.5, "2.5"),
            (1e15, "1000000000000000.0"),
            (0.0001, "0.0001"),
            (1.234_567_890_123_45, "1.23456789012345"),
            (1.0 / 3.0, "0.3333333333333333"),
            (-1.5, "-1.5"),
            (1e100, "1e+100"),
            (5e-324, "5e-324"),
            (123.456, "123.456"),
        ] {
            assert_eq!(float_repr(value), expected, "{value}");
        }
    }

    #[test]
    fn complex_numbers_print_like_python_repr() {
        assert_eq!(complex_repr(0.0, 1.0), "1j");
        assert_eq!(complex_repr(1.0, 2.0), "(1+2j)");
        assert_eq!(complex_repr(-0.0, -1.0), "(-0-1j)");
        assert_eq!(complex_repr(0.0, 10.5), "10.5j");
        assert_eq!(complex_repr(1.5, -1e16), "(1.5-1e+16j)");
        assert_eq!(complex_repr(0.0, 0.0), "0j");
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
    fn digits_are_what_str_isdigit_says_on_every_supported_python() {
        for (c, expected) in [
            ('0', Some(true)),
            ('a', Some(false)),
            ('_', Some(false)),
            ('\u{b2}', Some(true)),    // ²
            ('\u{663}', Some(true)),   // ٣
            ('\u{2460}', Some(true)),  // ①
            ('\u{ff19}', Some(true)),  // ９
            ('\u{bd}', Some(false)),   // ½
            ('\u{3007}', Some(false)), // 〇
            ('\u{216b}', Some(false)), // Ⅻ
            ('\u{4e00}', Some(false)), // 一, numeric but not `Nd`/`Nl`/`No`
            ('\u{df}', Some(false)),   // ß
            ('\u{16ac0}', None),       // a Tangsa digit: Python 3.11+ only
        ] {
            assert_eq!(is_digit(c), expected, "{c:?}");
        }
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

    #[test]
    fn digits_match_the_python_running_the_tests() {
        let venv = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.venv/bin/python");
        let python = std::env::var_os("AYE_TEST_PYTHON").map_or(venv, std::path::PathBuf::from);
        let Ok(output) = std::process::Command::new(&python)
            .args([
                "-c",
                "import sys; sys.stdout.write(' '.join(str(c) for c in range(0x110000) if chr(c).isdigit()))",
            ])
            .output()
        else {
            eprintln!("skipped: no Python");
            return;
        };
        if !output.status.success() {
            eprintln!("skipped: no Python");
            return;
        }
        let digits: std::collections::HashSet<u32> = String::from_utf8(output.stdout)
            .unwrap()
            .split_whitespace()
            .map(|c| c.parse().unwrap())
            .collect();
        for code in 0..0x11_0000 {
            let Some(c) = char::from_u32(code) else {
                continue;
            };
            if let Some(digit) = is_digit(c) {
                assert_eq!(digit, digits.contains(&code), "U+{code:04X}");
            }
        }
    }

    #[test]
    fn integer_literals_in_any_base() {
        assert_eq!(int_literal_decimal("100_000").as_deref(), Some("100000"));
        assert_eq!(int_literal_decimal("0x_ff").as_deref(), Some("255"));
        assert_eq!(int_literal_decimal("0o17").as_deref(), Some("15"));
        assert_eq!(int_literal_decimal("0b101").as_deref(), Some("5"));
        assert_eq!(
            int_literal_decimal("0xFFFFFFFFFFFFFFFFFFFF").as_deref(),
            Some("1208925819614629174706175")
        );
        assert_eq!(
            int_literal_decimal("100000000000000000000").as_deref(),
            Some("100000000000000000000")
        );
        assert_eq!(int_literal_decimal("0").as_deref(), Some("0"));
    }
}
