//! A test case's `classname` and `name`, from its id, as pytest writes
//! them ([`names`]).

/// `tests/test_cart.py::TestCart::test_add[eur]` → (`tests.test_cart.TestCart`,
/// `test_add[eur]`).
pub(super) fn names(id: &str) -> (String, String) {
    let (path, rest) = match id.split_once("::") {
        Some((path, rest)) => (path, Some(rest)),
        None => (id, None),
    };
    let module = path.strip_suffix(".py").unwrap_or(path).replace('/', ".");
    let Some(rest) = rest else {
        return (String::new(), module);
    };
    let (names, params) = rest.split_at(rest.find('[').unwrap_or(rest.len()));
    let mut parts: Vec<&str> = names.split("::").collect();
    let test = parts.pop().unwrap_or_default();
    let classname = std::iter::once(module.as_str())
        .chain(parts)
        .collect::<Vec<_>>()
        .join(".");
    (classname, format!("{test}{params}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_pytest() {
        let split = |id| names(id);
        assert_eq!(
            split("tests/test_cart.py::TestCart::test_add[eur]"),
            ("tests.test_cart.TestCart".into(), "test_add[eur]".into())
        );
        assert_eq!(
            split("test_x.py::test_y[a::b-c[1]]"),
            ("test_x".into(), "test_y[a::b-c[1]]".into())
        );
        assert_eq!(
            split("pkg/mod.py::pkg.mod.func"),
            ("pkg.mod".into(), "pkg.mod.func".into())
        );
        assert_eq!(
            split("tests/test_bad.py"),
            (String::new(), "tests.test_bad".into())
        );
    }
}
