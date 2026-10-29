//! The process environment, as aye read it.

use std::ffi::{OsStr, OsString};

/// Whether names are looked up whatever their case: on Windows, as both
/// the platform and Python's `os.environ` do.
const IGNORE_CASE: bool = cfg!(windows);

/// The process environment as aye read it: every variable, in the order
/// the platform gave them, each name's case kept.
///
/// A name is looked up as Python's `os.environ` looks one up, since what
/// aye reads it for is what pytest would read: on Windows whatever the
/// case of the name, and never a name `os.environ` can't hold (empty, or
/// holding `=` or NUL), which is set nowhere. Where a name repeats, the
/// first one counts, as for the platform's `getenv` and for `os.environ`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessEnv {
    vars: Vec<(OsString, OsString)>,
}

impl ProcessEnv {
    /// An environment holding no variable.
    pub const fn new() -> Self {
        Self { vars: Vec::new() }
    }

    /// This process's environment, as it is now.
    pub fn from_process() -> Self {
        Self::from_vars(std::env::vars_os())
    }

    /// An environment holding `vars`, in order.
    pub fn from_vars<K, V>(vars: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<OsString>,
        V: Into<OsString>,
    {
        Self {
            vars: vars
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
        }
    }

    /// The value of `name`, when it is set and is Unicode, as
    /// `std::env::var` gives it.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.get_os(name)?.to_str()
    }

    /// The value of `name`, when it is set.
    pub fn get_os(&self, name: &str) -> Option<&OsStr> {
        if name.is_empty() || name.contains(['=', '\0']) {
            return None;
        }
        self.vars
            .iter()
            .find(|(key, _)| same_name(key, name, IGNORE_CASE))
            .map(|(_, value)| value.as_os_str())
    }

    /// Every variable, in order, its name as read.
    pub fn iter(&self) -> impl Iterator<Item = (&OsStr, &OsStr)> {
        self.vars
            .iter()
            .map(|(name, value)| (name.as_os_str(), value.as_os_str()))
    }

    pub const fn len(&self) -> usize {
        self.vars.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }
}

/// Whether the variable `key` is the one `name` asks for. Ignoring case,
/// both are compared in upper case, as `os.environ` keys them.
fn same_name(key: &OsStr, name: &str, ignore_case: bool) -> bool {
    if ignore_case {
        key.to_str()
            .is_some_and(|key| key.to_uppercase() == name.to_uppercase())
    } else {
        key == name
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::{ProcessEnv, same_name};

    #[test]
    fn a_variable_is_found_by_its_name() {
        let env = ProcessEnv::from_vars([("HOME", "/home/me"), ("EMPTY", ""), ("HOME", "/other")]);
        assert_eq!(env.get("HOME"), Some("/home/me"));
        assert_eq!(env.get("EMPTY"), Some(""));
        assert_eq!(env.get("MISSING"), None);
        assert_eq!(env.len(), 3);
        assert_eq!(
            env.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            ["HOME", "EMPTY", "HOME"]
        );
    }

    #[test]
    fn names_os_environ_cannot_hold_are_set_nowhere() {
        let env = ProcessEnv::from_vars([("=C:", "C:\\"), ("A=B", "c"), ("", "x")]);
        for name in ["=C:", "A=B", "", "A\0B"] {
            assert_eq!(env.get(name), None, "{name:?}");
        }
    }

    #[test]
    fn case_counts_except_on_windows() {
        assert!(same_name(OsStr::new("Path"), "PATH", true));
        assert!(same_name(OsStr::new("straße"), "STRASSE", true));
        assert!(!same_name(OsStr::new("Path"), "PATH", false));
        assert!(same_name(OsStr::new("PATH"), "PATH", false));
        let env = ProcessEnv::from_vars([("Path", "/bin")]);
        assert_eq!(env.get("PATH").is_some(), cfg!(windows));
    }

    #[cfg(unix)]
    #[test]
    fn a_value_that_is_not_unicode_is_only_an_os_string() {
        use std::os::unix::ffi::OsStrExt;

        let value = OsStr::from_bytes(b"\xff");
        let env = ProcessEnv::from_vars([("BYTES", value)]);
        assert_eq!(env.get("BYTES"), None);
        assert_eq!(env.get_os("BYTES"), Some(value));
    }
}
