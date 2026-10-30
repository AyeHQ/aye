//! Every grace period and poll interval of the runner, each documented
//! once.

use std::time::Duration;

/// How long a worker may take to exit after `shutdown` before it is killed.
pub(crate) const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
/// How long a worker may take to exit once its protocol stream has ended.
pub(crate) const EXIT_GRACE: Duration = Duration::from_secs(2);
/// How long to wait for a killed worker to be gone.
pub(crate) const KILL_WAIT: Duration = Duration::from_secs(5);
/// How long to wait for the rest of stderr once the worker has exited.
/// Bounded, because a subprocess the tests left behind may hold it open.
pub(crate) const STDERR_GRACE: Duration = Duration::from_secs(1);
/// After a clean `shutdown`, how long stderr may stay silent before it
/// counts as ended: nothing goes into a report then, and a subprocess the
/// tests left running, which holds the stream open, would otherwise delay
/// the end of every run by [`STDERR_GRACE`]. A crash report waits for the
/// whole grace instead, so as not to lose the end of the crash's dump.
pub(crate) const STDERR_QUIET: Duration = Duration::from_millis(100);
/// How often a blocked read checks whether the worker is still alive.
pub(crate) const EXIT_POLL: Duration = Duration::from_millis(100);
/// Once the worker has exited, how long its stream may stay silent before it
/// counts as ended. What is left in the pipe arrives at once, and the stream
/// ends by itself unless a process the tests started holds the pipe open,
/// which may be forever. Generous, so that a reader thread starved of CPU
/// doesn't lose the worker's last lines.
pub(crate) const DRAIN: Duration = Duration::from_secs(1);
