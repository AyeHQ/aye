//! A line saying aye is collecting, for the work before the event stream
//! starts: preparing the adapters can walk a big project. Shown on a
//! terminal once the work has taken half a second, and cleared as soon as it
//! is done, so a quick start shows nothing. Collection's own progress then
//! comes through the stream (`collection_progress`).

use std::io::{self, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const AFTER: Duration = Duration::from_millis(500);
const LINE: &str = "collecting…";

pub struct Busy {
    done: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Busy {
    /// Start; nothing is ever written unless `live` (text output on a
    /// terminal).
    pub fn start(live: bool) -> Self {
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
