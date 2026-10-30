//! What a stop signal does to a run ([`install_interrupt`]): the first
//! stops it cleanly, the same one delivered again is ignored ([`Presses`]),
//! a later one ends aye at once.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use aye_events::ExitCode;

use crate::printer::Printer;

/// How long a stop signal before the run (while collecting) may take to
/// end aye cleanly before it exits anyway: whatever runs then (a walk, an
/// import in a worker) may not be watching.
const COLLECTING_GRACE: Duration = Duration::from_millis(1500);

/// Ctrl-C (or SIGTERM, or SIGHUP) stops the run cleanly; a second one
/// kills every worker, with the processes its tests started, and exits at
/// once. A signal within [`REPEAT_WINDOW`] of the first is that one
/// delivered again, not a second request (see [`Presses`]).
///
/// Before the run, a stop signal stops collection (`stop` the collector
/// watches), and aye exits anyway if that takes too long; during the run
/// (`running`), it stops the run cleanly.
#[expect(
    clippy::exit,
    reason = "a second signal, or a collection that doesn't stop in time, ends aye at once"
)]
pub(crate) fn install_interrupt(
    stop: aye_runner::StopHandle,
    collecting: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    printer: Printer,
) {
    let mut presses = Presses::default();
    let _ = ctrlc::set_handler(move || match presses.press(Instant::now()) {
        Press::Stop => {
            collecting.store(true, Ordering::SeqCst);
            stop.stop();
            if !running.load(Ordering::SeqCst) {
                let stop = stop.clone();
                let running = Arc::clone(&running);
                std::thread::spawn(move || {
                    std::thread::sleep(COLLECTING_GRACE);
                    if !running.load(Ordering::SeqCst) {
                        stop.kill();
                        printer.interrupted();
                        std::process::exit(i32::from(u8::from(ExitCode::Interrupted)));
                    }
                });
            }
        }
        Press::Repeat => {}
        Press::Kill => {
            stop.kill();
            std::process::exit(i32::from(u8::from(ExitCode::Interrupted)));
        }
    });
}

/// How long after the first stop signal another one is the same request
/// delivered twice: GNU `timeout` signals aye and then its own process
/// group, aye included, and a terminal and a wrapper passing signals on
/// (`uv run`) may each deliver one Ctrl-C. Shorter than a person takes to
/// see that the run is still stopping and ask again.
const REPEAT_WINDOW: Duration = Duration::from_millis(500);

/// What a stop signal means, by when it arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Press {
    /// The first: stop the run cleanly.
    Stop,
    /// Within [`REPEAT_WINDOW`] of the first: the same request again.
    Repeat,
    /// Later: a second request, to stop at once.
    Kill,
}

/// The stop signals seen so far.
#[derive(Debug, Default)]
struct Presses {
    first: Option<Instant>,
}

impl Presses {
    fn press(&mut self, now: Instant) -> Press {
        match self.first {
            None => {
                self.first = Some(now);
                Press::Stop
            }
            Some(first) if now.saturating_duration_since(first) < REPEAT_WINDOW => Press::Repeat,
            Some(_) => Press::Kill,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finding F40: GNU `timeout` signals aye, then its process group, aye
    /// included: two signals for one stop request, which must not kill the run
    /// before it reports.
    #[test]
    fn a_signal_delivered_twice_is_one_stop_request() {
        let start = Instant::now();
        let mut presses = Presses::default();
        assert_eq!(presses.press(start), Press::Stop);
        // The same request again, however often it arrives within the window.
        assert_eq!(presses.press(start), Press::Repeat);
        assert_eq!(
            presses.press(start + Duration::from_millis(1)),
            Press::Repeat
        );
        assert_eq!(
            presses.press(start + REPEAT_WINDOW.saturating_sub(Duration::from_millis(1))),
            Press::Repeat
        );
        // A second request, from someone who saw the run still stopping.
        assert_eq!(presses.press(start + REPEAT_WINDOW), Press::Kill);
        assert_eq!(presses.press(start + Duration::from_secs(5)), Press::Kill);
        // A clock that seems to go back is no second request.
        let mut presses = Presses::default();
        let later = start + Duration::from_secs(1);
        assert_eq!(presses.press(later), Press::Stop);
        assert_eq!(presses.press(start), Press::Repeat);
    }
}
