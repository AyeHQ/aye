//! What `aye show` feeds the renderers of a stored log, over its index
//! (`aye_stream::RunIndex`): the records of the results shown ([`feed`]),
//! with what it tells them of the run beside the log ([`context`],
//! [`stale`]).

use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aye_events::{Counts, Event, ExitCode, ProjectPath, Record, WorkerIndex};
use aye_history::RunMeta;
use aye_render::{ShowContext, ShowMode};
use aye_stream::{FileChange, ItemKind, RecordIndex, RunIndex, RunItem};

/// The records the renderer gets: the run's `run_started`, what `mode`
/// shows of the results `shown` (in details, with the `worker_started` of
/// the workers that ran them), and a `run_finished`: the run's own when
/// the list is the whole run (nothing `filtered`), else one counting the
/// tests shown, with the run's exit code (`exit_code`, the stored one when
/// the log has no `run_finished`).
pub(super) fn feed(
    index: &RunIndex<'_>,
    shown: &[&RunItem<'_>],
    mode: ShowMode,
    filtered: bool,
    exit_code: Option<ExitCode>,
) -> Vec<Record> {
    let mut places: BTreeSet<RecordIndex> = BTreeSet::new();
    for item in shown {
        places.insert(item.at);
        if mode == ShowMode::List {
            continue;
        }
        match &item.kind {
            ItemKind::Test(result, parts) => {
                places.extend(parts);
                if let Some(cause) = &result.cause {
                    places.extend(index.failures(cause));
                }
            }
            ItemKind::Lifetime(lifetime) => places.extend(index.failures(lifetime)),
            ItemKind::Diagnostic | ItemKind::Worker => {}
        }
    }
    if mode == ShowMode::Details {
        // The `worker_started` of each worker that ran a result shown: it
        // says which Python ran it.
        let workers: HashSet<WorkerIndex> = places
            .iter()
            .filter_map(|at| index.record(*at).envelope.worker)
            .collect();
        places.extend(
            index
                .records()
                .iter_enumerated()
                .filter_map(|(at, record)| {
                    (matches!(record.event, Event::WorkerStarted { .. })
                        && record
                            .envelope
                            .worker
                            .is_some_and(|worker| workers.contains(&worker)))
                    .then_some(at)
                }),
        );
    }
    let mut feed: Vec<Record> = index.run_started.into_iter().cloned().collect();
    feed.extend(places.into_iter().map(|at| index.record(at).clone()));
    let whole = mode == ShowMode::List && !filtered;
    match index.run_finished {
        Some(finished) if whole => feed.push(finished.clone()),
        _ => {
            let mut counts = Counts::default();
            let mut duration = Duration::ZERO;
            for item in shown {
                if let ItemKind::Test(result, _) = &item.kind {
                    counts.record(result.outcome);
                    duration = duration.saturating_add(result.duration);
                }
            }
            let exit_code = index
                .run_finished
                .and_then(|finished| match &finished.event {
                    Event::RunFinished { exit_code, .. } => Some(*exit_code),
                    _ => None,
                })
                .or(exit_code)
                .unwrap_or(if counts.bad() > 0 {
                    ExitCode::TestsFailed
                } else {
                    ExitCode::Ok
                });
            let mut record = Record::new(Event::RunFinished {
                exit_code,
                counts,
                duration,
                stopped: None,
            });
            record.envelope.run = index
                .run_started
                .and_then(|started| started.envelope.run.clone());
            feed.push(record);
        }
    }
    feed
}

/// The files of the results shown that changed since the run, each once,
/// in the order their results come.
pub(super) fn stale(
    meta: &RunMeta,
    root: &Path,
    shown: &[&RunItem<'_>],
) -> Vec<(ProjectPath, FileChange)> {
    let mut seen = HashSet::new();
    shown
        .iter()
        .map(|item| item.name.path())
        .filter(|path| !path.as_str().is_empty() && seen.insert(*path))
        .filter_map(|path| Some((path.to_project_path(), meta.change(root, path.as_str())?)))
        .collect()
}

/// What the renderers are told of the run `meta` beside its log, at
/// `now_ms`: its number, how long ago it finished, and the files of the
/// results shown that changed since (`stale`), each with how.
pub(super) fn context(
    meta: &RunMeta,
    stale: &[(ProjectPath, FileChange)],
    now_ms: u64,
) -> ShowContext {
    ShowContext {
        run: meta.number(),
        ago: (meta.finished_ms > 0).then(|| ago(now_ms, meta.finished_ms)),
        stale: stale.iter().cloned().collect(),
    }
}

/// How long before `now_ms` the time `then_ms` was (both in milliseconds
/// since the Unix epoch): `just now` (under a minute, or later than now),
/// `2 min ago`, `3 h ago`, `1 day ago`, `2 days ago`.
pub(super) fn ago(now_ms: u64, then_ms: u64) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        ..60 => "just now".to_owned(),
        60..3_600 => format!("{} min ago", secs / 60),
        3_600..86_400 => format!("{} h ago", secs / 3_600),
        86_400..172_800 => "1 day ago".to_owned(),
        _ => format!("{} days ago", secs / 86_400),
    }
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}
