//! Lifetimes: the scope a resource lives for and a lifetime opens.

use crate::serde_ext::string_enum;

string_enum! {
    /// How long something set up for tests lives: a resource's scope
    /// (pytest's fixture scopes), and the kind of a lifetime, which is the
    /// scope it opens (a lifetime never opens `function`). It orders widest
    /// first, as a resource may depend only on resources of its own scope or
    /// wider ones, and a scope this aye doesn't know comes last.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum Scope {
        /// The framework's session: once per worker, and kept from one batch
        /// to the next.
        Session = "session",
        /// A package and what lies below it.
        Package = "package",
        /// A module.
        Module = "module",
        /// A class.
        Class = "class",
        /// One test.
        Function = "function",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each scope is its lower-case name, widest first; one no variant
    /// spells keeps its text and comes last.
    #[test]
    fn scopes_are_their_names_widest_first() {
        let names: Vec<&str> = Scope::ALL.iter().map(Scope::as_str).collect();
        assert_eq!(names, ["session", "package", "module", "class", "function"]);
        assert!(Scope::ALL.is_sorted());
        for scope in Scope::ALL {
            let json = serde_json::to_string(scope).unwrap();
            assert_eq!(json, format!("\"{scope}\""));
            assert_eq!(serde_json::from_str::<Scope>(&json).unwrap(), *scope);
            assert_eq!(Scope::from(scope.as_str()), *scope);
        }
        let other: Scope = serde_json::from_str("\"Dynamic\"").unwrap();
        assert_eq!(other, Scope::Other("Dynamic".into()));
        assert_eq!(serde_json::to_string(&other).unwrap(), "\"Dynamic\"");
        assert!(Scope::Function < other);
    }
}
