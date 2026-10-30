//! What interpreter was asked for ([`PythonRequest`]), and where the
//! request came from ([`RequestSource`]).

use std::path::{Path, PathBuf};

/// An interpreter asked for by its user, as a path or a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PythonRequest {
    /// A file, which need not exist: starting it then fails with its name.
    File(PathBuf),
    /// A bare name (`python3.12`), looked up on `PATH`.
    ExecutableName(String),
}

impl PythonRequest {
    /// `value` as given where `base` is the directory relative paths start
    /// from: one component without a root is a name (when it is Unicode),
    /// anything else a file, joined to `base` when it is relative.
    pub fn parse(value: &Path, base: &Path) -> Self {
        let is_name = value.components().count() == 1 && !value.has_root();
        if is_name && let Some(name) = value.to_str() {
            return Self::ExecutableName(name.to_string());
        }
        Self::File(base.join(value))
    }
}

/// Where a [`PythonRequest`] came from. A request from the command line or
/// the environment is relative to the current directory, one from the
/// configuration to the project root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestSource {
    /// `--python`.
    Flag,
    /// `$AYE_PYTHON`.
    Env,
    /// `[tool.aye] python`.
    Config,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_name_is_a_name_and_anything_else_a_file() {
        let base = Path::new("/work");
        assert_eq!(
            PythonRequest::parse(Path::new("python3.12"), base),
            PythonRequest::ExecutableName("python3.12".to_string())
        );
        assert_eq!(
            PythonRequest::parse(Path::new("bin/python"), base),
            PythonRequest::File(base.join("bin/python"))
        );
        let absolute = std::env::temp_dir().join("python");
        assert_eq!(
            PythonRequest::parse(&absolute, base),
            PythonRequest::File(absolute.clone())
        );
    }
}
