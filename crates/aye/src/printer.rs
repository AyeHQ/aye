//! What the binary writes itself, outside the event stream ([`Printer`]):
//! its errors and warnings (`aye: error: …`), the `collecting…` line while
//! the adapters prepare ([`Busy`]), the word that a stop signal ended it,
//! and the text of `aye config`, `aye show --runs` and `aye show --log`.
//! The renderers write the stream's records themselves.

use std::fmt::Display;
use std::io::{self, IsTerminal as _, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use aye_config::OutputFormat;

/// Where the binary's own output goes: one value, handed to whatever
/// writes some, so that every such line is written here.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Printer;

#[expect(
    clippy::print_stderr,
    reason = "aye's own messages, outside the event stream"
)]
#[expect(
    clippy::unused_self,
    reason = "what writes aye's own output is handed the printer, which holds nothing yet"
)]
impl Printer {
    /// `aye: error: {message}`, on stderr: what stops aye, or a report
    /// that failed.
    pub(crate) fn error(self, message: impl Display) {
        eprintln!("aye: error: {message}");
    }

    /// `aye: warning: {message}`, on stderr: what went wrong beside the
    /// run, and fails nothing.
    pub(crate) fn warning(self, message: impl Display) {
        eprintln!("aye: warning: {message}");
    }

    /// `aye: {message}`, on stderr: what isn't an error of the run (the
    /// output that can't be written; `aye show` finding no run).
    pub(crate) fn note(self, message: impl Display) {
        eprintln!("aye: {message}");
    }

    /// `aye: interrupted`, on a line of its own, on stderr: a stop signal
    /// ended aye before the run could report.
    pub(crate) fn interrupted(self) {
        eprintln!("\naye: interrupted");
    }

    /// The `collecting…` line on stdout, for the work before the stream
    /// starts, when the output is text on a terminal ([`Busy`]).
    pub(crate) fn collecting(self, format: OutputFormat) -> Busy {
        Busy::start(format == OutputFormat::Text && io::stdout().is_terminal())
    }

    /// stdout, for what the binary prints outside the stream.
    pub(crate) fn stdout(self) -> io::Stdout {
        io::stdout()
    }
}

const AFTER: Duration = Duration::from_millis(500);
const LINE: &str = "collecting…";

/// A line saying aye is collecting, for the work before the event stream
/// starts: preparing the adapters can walk a big project. Shown on a
/// terminal once the work has taken half a second, and cleared as soon as
/// it is done (when this is dropped), so a quick start shows nothing.
/// Collection's own progress then comes through the stream
/// (`collection_progress`).
pub(crate) struct Busy {
    done: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Busy {
    /// Start; nothing is ever written unless `live`.
    fn start(live: bool) -> Self {
        let done = Arc::new((Mutex::new(false), Condvar::new()));
        let thread = live.then(|| {
            let done = Arc::clone(&done);
            thread::spawn(move || {
                let (lock, signal) = &*done;
                let guard = lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let (guard, _) = signal
                    .wait_timeout_while(guard, AFTER, |done| !*done)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if *guard {
                    return;
                }
                let mut out = io::stdout();
                let _ = write!(out, "\r{LINE}");
                let _ = out.flush();
                let guard = signal
                    .wait_while(guard, |done| !*done)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                drop(guard);
                let _ = write!(out, "\r{:width$}\r", "", width = LINE.chars().count());
                let _ = out.flush();
            })
        });
        Self { done, thread }
    }
}

impl Drop for Busy {
    /// The work is done: the line, if shown, is cleared before this returns.
    fn drop(&mut self) {
        let (lock, signal) = &*self.done;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        signal.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
