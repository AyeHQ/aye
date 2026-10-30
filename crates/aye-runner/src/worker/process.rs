//! A worker's process: killing it with its process group, or interrupting
//! it alone, from any thread.

use std::io;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};

use crate::sync::lock;

/// The worker's process. Everything that reaps it goes through here, so its
/// process group is only ever signalled while its pid can't have been
/// reused: until it is reaped, a dead worker's pid stays taken.
#[derive(Debug)]
pub(crate) struct Proc {
    pub(crate) child: Child,
    pub(crate) reaped: bool,
}

impl Proc {
    pub(super) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let polled = self.child.try_wait();
        if matches!(polled, Ok(Some(_))) {
            self.reaped = true;
        }
        polled
    }

    /// Kill the worker and its process group, unless it was reaped already
    /// (it exited by itself; what its tests left running is theirs).
    pub(crate) fn kill(&mut self) {
        if !self.reaped {
            kill_groups(&[self.child.id()]);
            let _ = self.child.kill();
        }
    }

    /// Interrupt the worker alone, as Ctrl-C interrupts pytest: SIGINT, a
    /// `KeyboardInterrupt` in Python, which ends its batch, tearing down.
    /// Where there is no such signal (Windows), kill it.
    fn interrupt(&mut self) {
        if self.reaped {
            return;
        }
        #[cfg(unix)]
        {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "kill -INT \"$1\"", "sh", &self.child.id().to_string()]);
            quietly(&mut command);
        }
        #[cfg(not(unix))]
        self.kill();
    }
}

/// A handle that kills or interrupts a worker from any thread.
#[derive(Debug, Clone)]
pub struct Killer(pub(super) Arc<Mutex<Proc>>);

impl Killer {
    /// Kill the worker and its process group.
    pub fn kill(&self) {
        lock(&self.0).kill();
    }

    /// Interrupt the worker alone (SIGINT), so that its framework ends the
    /// batch and tears down; kill it where there is no such signal.
    pub fn interrupt(&self) {
        lock(&self.0).interrupt();
    }
}

/// Kill each worker's process group: the worker and whatever its tests
/// started that stayed in the group (a worker leads its own, see
/// [`own_process_group`]). std can only signal a single process and the
/// workspace forbids the unsafe code `killpg` takes, so the shell's `kill`
/// sends the signals. On Windows, `taskkill /T` ends each worker's process
/// tree. The worker itself is killed directly as well, in case this fails.
pub(crate) fn kill_groups(pids: &[u32]) {
    if pids.is_empty() {
        return;
    }
    #[cfg(unix)]
    {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "kill -9 \"$@\"", "sh"])
            .args(pids.iter().map(|pid| format!("-{pid}")));
        quietly(&mut command);
    }
    #[cfg(windows)]
    for pid in pids {
        let mut command = Command::new("taskkill");
        command.args(["/F", "/T", "/PID", &pid.to_string()]);
        quietly(&mut command);
    }
}

#[cfg(any(unix, windows))]
fn quietly(command: &mut Command) {
    let _ = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Workers get their own process group, so the terminal's Ctrl-C reaches
/// only aye, which then stops them (see `StopHandle`), and so that killing
/// a worker can kill what its tests started (see [`kill_groups`]).
pub(super) fn own_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(not(any(unix, windows)))]
    let _ = command;
}
