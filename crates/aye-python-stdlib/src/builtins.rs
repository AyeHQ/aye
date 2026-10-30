//! Python's builtins, as every Python aye supports has them (3.10 to 3.14):
//! where they differ between versions, an answer says it can't tell.

/// Builtins that rebind names or reach the namespace by name.
pub const DYNAMIC_CALLS: &[&str] = &[
    "exec",
    "eval",
    "globals",
    "locals",
    "vars",
    "setattr",
    "delattr",
    "__import__",
];

/// Builtins that only read what they are given.
pub const READING_BUILTINS: &[&str] = &[
    "print",
    "isinstance",
    "issubclass",
    "len",
    "repr",
    "str",
    "id",
    "hash",
    "hasattr",
    "getattr",
    "callable",
    "type",
    "list",
    "tuple",
    "dict",
    "set",
    "frozenset",
    "sorted",
    "iter",
    "any",
    "all",
    "vars",
    "dir",
];

/// Python's `str.isdigit` (Unicode `Numeric_Type` `Digit` or `Decimal`) of
/// one character, on every Python aye supports: 3.10 to 3.14, whose
/// Unicode databases go from 13.0 to 16.0. `None` when they disagree, or
/// for a numeric character the tables below don't cover (Rust's Unicode is
/// newer).
pub fn str_isdigit(c: char) -> Option<bool> {
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

/// Python builtins that have a `__name__`, with that name: those of every
/// Python aye supports (3.10 to 3.14). `ExceptionGroup` and
/// `BaseExceptionGroup` (3.11) and `PythonFinalizationError` (3.13) are
/// left out: whether they exist depends on the version.
pub fn builtin_name(name: &str) -> Option<&'static str> {
    const BUILTINS: &[&str] = &[
        "ArithmeticError",
        "AssertionError",
        "AttributeError",
        "BaseException",
        "BlockingIOError",
        "BrokenPipeError",
        "BufferError",
        "BytesWarning",
        "ChildProcessError",
        "ConnectionAbortedError",
        "ConnectionError",
        "ConnectionRefusedError",
        "ConnectionResetError",
        "DeprecationWarning",
        "EOFError",
        "EncodingWarning",
        "Exception",
        "FileExistsError",
        "FileNotFoundError",
        "FloatingPointError",
        "FutureWarning",
        "GeneratorExit",
        "ImportError",
        "ImportWarning",
        "IndentationError",
        "IndexError",
        "InterruptedError",
        "IsADirectoryError",
        "KeyError",
        "KeyboardInterrupt",
        "LookupError",
        "MemoryError",
        "ModuleNotFoundError",
        "NameError",
        "NotADirectoryError",
        "NotImplementedError",
        "OSError",
        "OverflowError",
        "PendingDeprecationWarning",
        "PermissionError",
        "ProcessLookupError",
        "RecursionError",
        "ReferenceError",
        "ResourceWarning",
        "RuntimeError",
        "RuntimeWarning",
        "StopAsyncIteration",
        "StopIteration",
        "SyntaxError",
        "SyntaxWarning",
        "SystemError",
        "SystemExit",
        "TabError",
        "TimeoutError",
        "TypeError",
        "UnboundLocalError",
        "UnicodeDecodeError",
        "UnicodeEncodeError",
        "UnicodeError",
        "UnicodeTranslateError",
        "UnicodeWarning",
        "UserWarning",
        "ValueError",
        "Warning",
        "ZeroDivisionError",
        "abs",
        "aiter",
        "all",
        "anext",
        "any",
        "ascii",
        "bin",
        "bool",
        "breakpoint",
        "bytearray",
        "bytes",
        "callable",
        "chr",
        "classmethod",
        "compile",
        "complex",
        "delattr",
        "dict",
        "dir",
        "divmod",
        "enumerate",
        "eval",
        "exec",
        "filter",
        "float",
        "format",
        "frozenset",
        "getattr",
        "globals",
        "hasattr",
        "hash",
        "hex",
        "id",
        "input",
        "int",
        "isinstance",
        "issubclass",
        "iter",
        "len",
        "list",
        "locals",
        "map",
        "max",
        "memoryview",
        "min",
        "next",
        "object",
        "oct",
        "open",
        "ord",
        "pow",
        "print",
        "property",
        "range",
        "repr",
        "reversed",
        "round",
        "set",
        "setattr",
        "slice",
        "sorted",
        "staticmethod",
        "str",
        "sum",
        "super",
        "tuple",
        "type",
        "vars",
        "zip",
    ];
    match name {
        "EnvironmentError" | "IOError" => Some("OSError"),
        _ => BUILTINS.iter().find(|b| **b == name).copied(),
    }
}

/// `float(text)` for the spellings Python accepts, ASCII only (Python also
/// takes other Unicode digits and spaces, which are left to it):
/// surrounding whitespace, one sign, then `inf`, `infinity`, `nan`, or
/// digits with single underscores between them, an optional fraction and an
/// optional exponent.
pub fn parse_float(text: &str) -> Option<f64> {
    /// Digits from `*at`, with single underscores between digits: how many
    /// digits, or `None` for a misplaced underscore.
    fn digits(bytes: &[u8], at: &mut usize) -> Option<usize> {
        let mut count = 0;
        while let Some(&byte) = bytes.get(*at) {
            if byte.is_ascii_digit() {
                count += 1;
            } else if byte == b'_' {
                if count == 0 || !bytes.get(*at + 1).is_some_and(u8::is_ascii_digit) {
                    return None;
                }
            } else {
                break;
            }
            *at += 1;
        }
        Some(count)
    }

    let trimmed = text.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c'));
    if !trimmed.is_ascii() {
        return None;
    }
    let (sign, rest) = match trimmed.as_bytes().first() {
        Some(b'-') => (-1.0, &trimmed[1..]),
        Some(b'+') => (1.0, &trimmed[1..]),
        _ => (1.0, trimmed),
    };
    match rest.to_ascii_lowercase().as_str() {
        "inf" | "infinity" => return Some(sign * f64::INFINITY),
        "nan" => return Some(f64::NAN),
        _ => {}
    }
    let bytes = rest.as_bytes();
    let mut at = 0;
    let whole = digits(bytes, &mut at)?;
    let mut fraction = 0;
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        fraction = digits(bytes, &mut at)?;
    }
    if whole == 0 && fraction == 0 {
        return None;
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        if digits(bytes, &mut at)? == 0 {
            return None;
        }
    }
    if at != bytes.len() {
        return None;
    }
    let cleaned: String = rest.chars().filter(|c| *c != '_').collect();
    cleaned.parse::<f64>().ok().map(|f| sign * f)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert_eq!(str_isdigit(c), expected, "{c:?}");
        }
    }

    #[test]
    fn digits_match_the_python_running_the_tests() {
        let Some(python) = aye_test::test_python(aye_test::Needs::Stdlib) else {
            return;
        };
        let output = std::process::Command::new(&python)
            .args([
                "-c",
                "import sys; sys.stdout.write(' '.join(str(c) for c in range(0x110000) if chr(c).isdigit()))",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", python.display());
        let digits: std::collections::HashSet<u32> = String::from_utf8(output.stdout)
            .unwrap()
            .split_whitespace()
            .map(|c| c.parse().unwrap())
            .collect();
        for code in 0..0x11_0000 {
            let Some(c) = char::from_u32(code) else {
                continue;
            };
            if let Some(digit) = str_isdigit(c) {
                assert_eq!(digit, digits.contains(&code), "U+{code:04X}");
            }
        }
    }

    #[test]
    fn floats_parse_as_python_parses_them() {
        for (text, expected) in [
            ("1.5", Some(1.5)),
            ("  -2e3\n", Some(-2000.0)),
            ("1_000.000_1", Some(1000.0001)),
            (".5", Some(0.5)),
            ("5.", Some(5.0)),
            ("+1E+05", Some(100_000.0)),
            ("007", Some(7.0)),
            ("Infinity", Some(f64::INFINITY)),
            ("-iNf", Some(f64::NEG_INFINITY)),
            ("", None),
            (".", None),
            ("e5", None),
            ("1e", None),
            ("1e+", None),
            ("_1", None),
            ("1_", None),
            ("1__0", None),
            ("1_.5", None),
            ("1._5", None),
            ("1_e5", None),
            ("1e_5", None),
            ("+-5", None),
            ("--5", None),
            ("0x10", None),
            ("1 5", None),
            ("infinit", None),
            ("nan(1)", None),
        ] {
            assert_eq!(parse_float(text), expected, "{text:?}");
        }
        assert!(parse_float("-nan").is_some_and(f64::is_nan));
    }
}
