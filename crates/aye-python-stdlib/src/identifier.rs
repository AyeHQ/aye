//! Python's identifiers, as `str.isidentifier` reads them, without
//! Unicode's `XID_Start` and `XID_Continue` tables: exactly for ASCII
//! names, approximately for the others.

/// `str.isidentifier()` of an ASCII name: a letter or `_`, then letters,
/// digits and `_`. No other name is one here, though Python takes many.
pub fn is_ascii_identifier(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `str.isidentifier()`, approximated with Unicode's alphabetic characters
/// for `XID_Start` and its alphanumeric ones for `XID_Continue`.
pub fn is_identifier(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_follow_str_isidentifier() {
        for name in ["x", "_", "_x1", "Test_A"] {
            assert!(is_ascii_identifier(name), "{name}");
            assert!(is_identifier(name), "{name}");
        }
        for name in ["", "1x", "x-y", "x.y", "x y"] {
            assert!(!is_ascii_identifier(name), "{name}");
            assert!(!is_identifier(name), "{name}");
        }
        assert!(!is_ascii_identifier("café"));
        assert!(is_identifier("café"));
    }
}
