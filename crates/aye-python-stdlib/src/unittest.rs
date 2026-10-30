//! unittest's loader rules, as `TestLoader.discover` applies them.

/// unittest's `VALID_MODULE_NAME`, `[_a-z]\w*\.py$` with `re.IGNORECASE`:
/// the files discovery may import. Under `IGNORECASE`, `[a-z]` also takes
/// `İ`, `ı`, `ſ` and the Kelvin sign; `\w` is a letter, a digit or `_`.
pub fn valid_module_name(name: &str) -> bool {
    let Some(stem) = name
        .len()
        .checked_sub(3)
        .filter(|&at| name.is_char_boundary(at) && name[at..].eq_ignore_ascii_case(".py"))
        .map(|at| &name[..at])
    else {
        return false;
    };
    let mut chars = stem.chars();
    let first_ok = chars.next().is_some_and(|c| {
        c == '_'
            || c.is_ascii_alphabetic()
            || matches!(c, '\u{130}' | '\u{131}' | '\u{17f}' | '\u{212a}')
    });
    first_ok && chars.all(|c| c == '_' || c.is_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_names_follow_valid_module_name() {
        for name in ["test_x.py", "_x.py", "T.PY", "ſx.py", "té.py", "x1.py"] {
            assert!(valid_module_name(name), "{name}");
        }
        for name in ["1x.py", "t-x.py", "t.x.py", ".py", "x.pyc", "x.p", "éx.py"] {
            assert!(!valid_module_name(name), "{name}");
        }
    }
}
