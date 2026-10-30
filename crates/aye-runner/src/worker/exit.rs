//! How a worker ended, and the words for it.

use std::process::ExitStatus;

/// How a worker ended.
#[derive(Debug, Clone)]
pub struct Exit {
    /// `None` when it couldn't be waited for.
    pub status: Option<ExitStatus>,
    /// The end of its stderr.
    pub stderr: String,
}

impl Exit {
    pub fn code(&self) -> Option<i32> {
        self.status.and_then(|status| status.code())
    }

    /// `exit status 3`, `killed by signal 11 (SIGSEGV)`.
    pub fn describe(&self) -> String {
        describe(self.status)
    }
}

/// What polling for a worker's exit found.
pub(super) enum Waited {
    Exited(ExitStatus),
    Running,
    /// It can't be waited for.
    Unknown,
}

/// `exit status 3`, `killed by signal 11 (SIGSEGV)`.
pub fn describe(status: Option<ExitStatus>) -> String {
    let Some(status) = status else {
        return "exit status unknown".to_string();
    };
    if let Some(code) = status.code() {
        return format!("exit status {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return match signal_name(signal) {
                Some(name) => format!("killed by signal {signal} ({name})"),
                None => format!("killed by signal {signal}"),
            };
        }
    }
    status.to_string()
}

/// Names of the signals whose numbers POSIX fixes.
#[cfg_attr(not(unix), allow(dead_code))]
fn signal_name(signal: i32) -> Option<&'static str> {
    Some(match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn exit_statuses_are_described() {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            describe(Some(ExitStatus::from_raw(3 << 8))),
            "exit status 3"
        );
        assert_eq!(
            describe(Some(ExitStatus::from_raw(11))),
            "killed by signal 11 (SIGSEGV)"
        );
        assert_eq!(
            describe(Some(ExitStatus::from_raw(31))),
            "killed by signal 31"
        );
        assert_eq!(describe(None), "exit status unknown");
    }
}
