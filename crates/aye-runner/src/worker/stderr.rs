//! A worker's stderr, drained on a thread: kept for crash reports, and
//! echoed live when output isn't captured (`-s`).

use std::io::{self, Read, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::sync::lock;

use super::Process;
use super::crash_report::report;

/// Bytes of stderr kept for crash reports.
const STDERR_KEEP: usize = 64 * 1024;

/// Where a worker's output is echoed when it isn't captured (`-s`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Echo {
    /// Our stdout, beside the text report.
    #[default]
    Stdout,
    /// Our stderr, so that a machine-readable stdout (`--output json`)
    /// stays one record per line.
    Stderr,
}

/// A worker's stderr, drained on a thread.
#[derive(Debug, Clone)]
pub(super) struct Stderr(Arc<(Mutex<Tail>, Condvar)>);

#[derive(Debug, Default)]
struct Tail {
    bytes: Vec<u8>,
    /// Where what is taken in is echoed, unless the reader was given its
    /// own writer.
    echo: Option<Echo>,
    done: bool,
    /// When the last bytes were taken in.
    last: Option<Instant>,
    /// Bytes are being taken in (echoed) right now.
    busy: bool,
}

impl Stderr {
    pub(super) fn drain(stream: impl Read + Send + 'static, echo: Option<Echo>) -> Self {
        let stderr = Self(Arc::default());
        lock(&stderr.0.0).echo = echo;
        let shared = stderr.clone();
        thread::spawn(move || shared.read(stream, None::<io::Sink>));
        stderr
    }

    /// Take the stream in until it ends, echoing it to `to` if given, else
    /// where [`Tail::echo`] says at the time.
    fn read(&self, mut stream: impl Read, mut to: Option<impl Write>) {
        let mut buf = [0u8; 8192];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let echo = {
                        let mut tail = lock(&self.0.0);
                        tail.busy = true;
                        tail.echo
                    };
                    let chunk = &buf[..n];
                    match (to.as_mut(), echo) {
                        (Some(out), _) => write_flushed(out, chunk),
                        (None, Some(Echo::Stdout)) => write_flushed(&mut io::stdout(), chunk),
                        (None, Some(Echo::Stderr)) => write_flushed(&mut io::stderr(), chunk),
                        (None, None) => {}
                    }
                    let mut tail = lock(&self.0.0);
                    tail.bytes.extend_from_slice(&buf[..n]);
                    if tail.bytes.len() > 2 * STDERR_KEEP {
                        let cut = tail.bytes.len() - STDERR_KEEP;
                        tail.bytes.drain(..cut);
                    }
                    tail.busy = false;
                    tail.last = Some(Instant::now());
                    drop(tail);
                    self.0.1.notify_all();
                }
            }
        }
        lock(&self.0.0).done = true;
        self.0.1.notify_all();
    }

    /// What goes into a report, once the worker has exited: waits up to
    /// `wait` for the stream to end first, but no longer than it stays
    /// silent for `quiet` (a process the tests started may hold it open for
    /// good). See [`report`].
    pub(super) fn tail(&self, wait: Duration, quiet: Duration) -> String {
        let (tail, changed) = &*self.0;
        let start = Instant::now();
        let deadline = start + wait;
        let mut guard = lock(tail);
        loop {
            let silent_since = guard.last.map_or(start, |last| last.max(start));
            let until = if guard.busy {
                deadline
            } else {
                (silent_since + quiet).min(deadline)
            };
            let now = Instant::now();
            if guard.done || now >= until {
                break;
            }
            guard = changed
                .wait_timeout(guard, until - now)
                .map_or_else(|poisoned| poisoned.into_inner().0, |(guard, _)| guard);
        }
        report(&String::from_utf8_lossy(&guard.bytes))
    }
}

fn write_flushed(out: &mut impl Write, bytes: &[u8]) {
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

impl Process {
    pub(super) fn stderr_tail(&self) -> String {
        self.stderr.tail(Duration::ZERO, Duration::ZERO)
    }

    pub(super) fn set_echo(&self, echo: Option<Echo>) {
        lock(&self.stderr.0.0).echo = echo;
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use crate::worker::crash_report::TAIL_BYTES;

    use super::*;

    #[test]
    fn stderr_is_echoed_and_its_tail_kept() {
        let stderr = Stderr(Arc::default());
        let mut echoed = Vec::new();
        let mut text = String::new();
        for i in 1..=100 {
            writeln!(text, "line {i}").unwrap();
        }
        stderr.read(text.as_bytes(), Some(&mut echoed));
        assert_eq!(echoed, text.as_bytes());
        let tail = stderr.tail(Duration::ZERO, Duration::ZERO);
        assert!(tail.starts_with("line 71\n"), "{tail}");
        assert!(tail.ends_with("line 100"), "{tail}");
    }

    #[test]
    fn the_kept_stderr_is_bounded() {
        let stderr = Stderr(Arc::default());
        let big = "é".repeat(3 * STDERR_KEEP);
        stderr.read(big.as_bytes(), None::<Vec<u8>>);
        assert!(lock(&stderr.0.0).bytes.len() <= 2 * STDERR_KEEP);
        let tail = stderr.tail(Duration::ZERO, Duration::ZERO);
        assert!(tail.len() <= TAIL_BYTES && tail.chars().all(|c| c == 'é'));
    }

    #[test]
    fn tail_waits_for_the_stream_to_end_but_not_forever() {
        let stderr = Stderr(Arc::default());
        let started = Instant::now();
        let wait = Duration::from_millis(30);
        assert_eq!(stderr.tail(wait, wait), "");
        assert!(started.elapsed() >= wait);
    }

    #[test]
    fn tail_stops_waiting_once_the_stream_is_quiet() {
        let stderr = Stderr(Arc::default());
        stderr.read("about to exit\n".as_bytes(), None::<Vec<u8>>);
        lock(&stderr.0.0).done = false; // a subprocess holds it open
        let started = Instant::now();
        let tail = stderr.tail(Duration::from_secs(10), Duration::from_millis(30));
        let elapsed = started.elapsed();
        assert_eq!(tail, "about to exit");
        assert!(
            elapsed >= Duration::from_millis(30) && elapsed < Duration::from_secs(5),
            "{elapsed:?}"
        );
    }

    #[test]
    fn tail_waits_while_bytes_are_being_taken_in_however_quiet() {
        let stderr = Stderr(Arc::default());
        lock(&stderr.0.0).busy = true; // echoing to a slow terminal
        let reader = stderr.clone();
        let echo = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            reader.read("the last line\n".as_bytes(), None::<Vec<u8>>);
        });
        let started = Instant::now();
        let tail = stderr.tail(Duration::from_secs(10), Duration::from_millis(10));
        assert!(started.elapsed() >= Duration::from_millis(200));
        assert_eq!(tail, "the last line");
        echo.join().unwrap();
    }
}
