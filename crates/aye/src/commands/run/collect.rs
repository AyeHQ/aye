//! A run's collection ([`collect`]): the collector over the prepared
//! adapters, what it walks, the refinement cache and what its key holds,
//! the runner refining what can't be predicted, and how collection
//! reports its progress while it runs ([`Pace`]).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use aye_adapter::Project;
use aye_collect::{Collection, Collector, Options};
use aye_events::Event;
use aye_fs::{StateBucket, StateDir};
use aye_protocol::Refiner;
use aye_provision::EnvironmentFingerprint;
use aye_runner::Runner;

use crate::settings::RunSettings;
use crate::stream::Stream;

/// What collection found, and when it started: `collection_finished`
/// says how long it took.
pub(super) struct Collected {
    pub(super) collection: Collection,
    pub(super) started: Instant,
}

/// Collect the project as `settings` say, from `collection_started` on,
/// with `runner` refining (unless `--no-refine`) and `collection_progress`
/// on `out` while it runs, until `stop` says to stop. `None` when a stop
/// signal interrupted it, which `collection_finished`, with nothing found,
/// has then said.
pub(super) fn collect(
    settings: &RunSettings,
    state: &StateDir,
    runner: &Runner,
    stop: &Arc<AtomicBool>,
    out: &mut Stream<'_>,
) -> Option<Collected> {
    out.emit(Event::CollectionStarted);
    let started = Instant::now();
    let project = settings.project();
    // The refinement cache is keyed on what refining depends on: the
    // interpreter and its packages, the arguments after `--`, and the
    // environment workers inherit (aye's own, as it read it; the
    // `PYTEST_XDIST_TESTRUNUID` the runner adds changes every run and
    // refinement has no use for it).
    let collector = Collector::new(
        project,
        &settings.prepared,
        Options {
            strict: settings.strict,
            cache_dir: Some(state.bucket(StateBucket::Cache)),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    )
    .with_environment(EnvironmentFingerprint {
        interpreter: settings.interpreter.value.executable.clone(),
        framework_args: settings.framework_args.clone(),
        variables: settings.env.vars.clone(),
    })
    .with_stop(Arc::clone(stop));
    let targets = targets(settings, project);
    let refiner: Option<&(dyn Refiner + Sync)> = if settings.no_refine {
        None
    } else {
        Some(runner)
    };
    let mut pace = Pace::new(started);
    let collection = collector.collect_watching(&targets, refiner, &mut |progress| {
        if pace.due() {
            out.emit(Event::CollectionProgress {
                walked: progress.walked,
                test_files: progress.test_files,
                refining: progress.refining,
            });
        }
    });
    if collection.interrupted {
        out.emit(Event::CollectionFinished {
            selected: 0,
            deselected: 0,
            refined_files: 0,
            duration: started.elapsed(),
        });
        return None;
    }
    Some(Collected {
        collection,
        started,
    })
}

/// What collection walks: the path arguments when every positional
/// argument is one; nothing given when there are none (each recognizer's
/// roots); and for paths mixed with dotted names or keys, the paths, and
/// wherever the adapters look when no path is given, for the rest.
fn targets(settings: &RunSettings, project: Project<'_>) -> Vec<PathBuf> {
    if settings.only_paths {
        return settings.paths.clone();
    }
    if settings.paths.is_empty() {
        return Vec::new();
    }
    let bare = Project {
        args: &[],
        ..project
    };
    let mut targets: Vec<PathBuf> = settings
        .prepared
        .iter()
        .flat_map(|(_, recognizer)| recognizer.roots(&bare))
        .filter(|root| root.exists())
        .collect();
    targets.extend(settings.paths.iter().cloned());
    targets
}

/// When collection reports its progress: not before it has taken half a
/// second, so that a quick collection sends nothing, then at most four times
/// a second.
struct Pace {
    started: Instant,
    last: Option<Instant>,
}

impl Pace {
    const AFTER: Duration = Duration::from_millis(500);
    const EVERY: Duration = Duration::from_millis(250);

    const fn new(started: Instant) -> Self {
        Self {
            started,
            last: None,
        }
    }

    fn due(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.started) < Self::AFTER
            || self
                .last
                .is_some_and(|last| now.duration_since(last) < Self::EVERY)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}
