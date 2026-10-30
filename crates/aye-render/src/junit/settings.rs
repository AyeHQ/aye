//! How the report is written: the framework's JUnit settings, as pytest
//! spells them, else aye's own ([`JunitOptions`]).

/// How the report is written: the framework's JUnit settings (pytest's
/// `junit_*` options, which the binary reads from the adapter's context),
/// else aye's own ([`JunitOptions::default`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JunitOptions {
    /// The `<testsuite>`'s name (pytest's `junit_suite_name`).
    pub suite_name: String,
    /// Which captured output a test case carries (`junit_logging`).
    pub logging: JunitLogging,
    /// Whether a test that didn't fail carries its output
    /// (`junit_log_passing_tests`).
    pub log_passing_tests: bool,
    /// What a test case's `time` counts (`junit_duration_report`).
    pub duration: JunitDuration,
    /// The schema, which says what attributes a test case has
    /// (`junit_family`).
    pub family: JunitFamily,
}

impl Default for JunitOptions {
    /// aye's own report: the suite named `aye`, every test's output, the
    /// whole test's time, and `file` and `line` on each test case.
    fn default() -> Self {
        Self {
            suite_name: "aye".to_string(),
            logging: JunitLogging::Aye,
            log_passing_tests: true,
            duration: JunitDuration::Total,
            family: JunitFamily::Xunit1,
        }
    }
}

/// Which captured output a test case carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JunitLogging {
    /// aye's own: stdout then the log in `<system-out>`, stderr in
    /// `<system-err>`, each only when there is some.
    Aye,
    /// pytest's `no`: none.
    No,
    /// pytest's `log`: the captured log in `<system-out>`.
    Log,
    /// pytest's `system-out`: stdout in `<system-out>`.
    SystemOut,
    /// pytest's `system-err`: stderr in `<system-err>`.
    SystemErr,
    /// pytest's `out-err`: stdout in `<system-out>`, stderr in
    /// `<system-err>`.
    OutErr,
    /// pytest's `all`: the log and stdout in `<system-out>`, stderr in
    /// `<system-err>`.
    All,
}

/// What a test case's `time` counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JunitDuration {
    /// Setup, body and teardown (pytest's `total`).
    Total,
    /// The body alone (pytest's `call`), where the adapter timed it apart;
    /// else the whole test.
    Body,
}

/// The schema of the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JunitFamily {
    /// A test case has `file` and `line` (pytest's `xunit1`, `legacy`).
    Xunit1,
    /// A test case has only `classname`, `name` and `time` (pytest's
    /// `xunit2`, its default).
    Xunit2,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pytests_spellings_of_the_settings() {
        assert_eq!(JunitLogging::from("all"), JunitLogging::All);
        assert_eq!(JunitLogging::from("out-err"), JunitLogging::OutErr);
        assert_eq!(JunitLogging::from("bogus"), JunitLogging::No);
        assert_eq!(JunitDuration::from("call"), JunitDuration::Body);
        assert_eq!(JunitDuration::from("total"), JunitDuration::Total);
        assert_eq!(JunitFamily::from("legacy"), JunitFamily::Xunit1);
        assert_eq!(JunitFamily::from("xunit1"), JunitFamily::Xunit1);
        assert_eq!(JunitFamily::from("xunit2"), JunitFamily::Xunit2);
    }
}
