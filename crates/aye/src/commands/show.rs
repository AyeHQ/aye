//! `aye show`: a stored run's results again, without running anything.
//!
//! It reads the run store (`.aye/runs`, `aye_history::store`) and feeds the
//! renderers part of a stored log, with the whole log's test keys
//! ([`Keys::of`]), so that what they write reads as the run wrote it, and
//! what it knows of the run beside the log ([`aye_render::ShowContext`]:
//! its number, how long ago it finished, which of the files shown changed
//! since).
//! What it shows are the log's results, as its index has them
//! ([`RunIndex`], [`RunItem`]): each test's result, and
//! what the run's summary counts beside them (a failed lifetime, an error
//! diagnostic such as a collection error, a module skipped while
//! collecting) or shows whatever the level (a failed worker).
//! - With no test named (paths alone filter it): the list, a line per
//!   result and the summary ([`ShowMode::List`]), the run's own
//!   `run_finished` when nothing is filtered out (so the summary is the
//!   run's), else one counting what is listed. A failed lifetime that kept
//!   tests from running is listed through their lines.
//! - With keys (or their start), ids or dotted names (and paths, which
//!   select their results then), or `--failed` (every bad outcome): their
//!   details, as a run of those tests would show them
//!   ([`ShowMode::Details`]): a test's `test_started`, `sub_outcome` and
//!   `test_finished` records, the failed `lifetime_finished` of the
//!   lifetimes that kept it from running; a failed lifetime's every
//!   failure; a diagnostic's record; then the run's `run_started` first and
//!   a `run_finished` counting them last. A key names a test's every
//!   result (a `duplicate-id` has several), or a lifetime's; the start of
//!   a key names the one test or lifetime whose key starts so, and the
//!   start of several is a usage error that lists them with their keys.
//!   A string of digits is a key's start like any other.
//!
//! The flags mean what they mean in a run; without `-v` or `-q`, details
//! are at the ladder's second level (text's `-v`, JSON's `-vv`), and the
//! list at a run's default. `--log` writes the stored log as it is, a
//! record per line, whatever the format.
//!
//! A `run_finished` it makes carries the stored run's exit code, as the
//! run's own does: the summary says how the run ended, and the process
//! exits 0 whenever it could show what was asked.
//!
//! A file of a result shown that changed since the run, or is gone, is
//! said first ([`Text::stale`], [`JsonLines::stale`]), and is in the
//! context's `stale`. Exit codes: 0 when it showed what was asked, whatever
//! the outcomes; 4 for what can't be shown (a selector naming nothing of
//! the run, with the closest names; the start of several keys, with them;
//! a bad expression or option; a run no stored run is); 5 when no run is
//! stored.
//!
//! Decided (implementation, October 2026):
//! - **`aye show` reads a kept run back** (detected as `aye config` is)
//!   and never runs anything: the list of a run's results (its tests, and
//!   its failed lifetimes, error diagnostics and modules skipped while
//!   collecting, so that the whole list ends with the run's own summary),
//!   or the details of the results named (by key, id, dotted name or path)
//!   or of every failure (`--failed`), through the same renderers, fed part
//!   of the log with the whole log's keys, what the store knows beside it
//!   and the run's exit code; or the stored log itself (`--log`). It needs
//!   no interpreter, and prepares no adapter. Its usage lines name it `aye
//!   show`.

use std::io::{self, Write as _};

use aye_config::OutputFormat;
use aye_history::{RunMeta, RunSelector, RunStore, SelectRunError, StoredRunId};
use aye_render::{JsonLines, RenderMode, ShowMode, Text, TextOptions};
use aye_stream::{Consumer, Keys, RunIndex, RunItem};

use crate::cli::ShowArgs;
use crate::error::AyeError;
use crate::printer::Printer;
use crate::settings::ShowSettings;

mod choose;
mod feed;
mod runs;

use choose::{Filter, Wanted, choose, read};
use feed::{context, feed, now_ms, stale};
use runs::list_runs;

/// Show what `args` ask for of a stored run, through `printer` where it
/// isn't a renderer's.
pub(crate) fn show(args: &ShowArgs, printer: Printer) -> Result<(), AyeError> {
    let settings = ShowSettings::resolve(args).map_err(AyeError::usage)?;
    let (cwd, root, format) = (&settings.cwd, &settings.root, settings.format);
    let store = RunStore::new(aye_fs::StateDir::for_root(root).bucket(aye_fs::StateBucket::Runs));
    let keep = settings.keep_runs;
    if args.runs {
        return list_runs(&store, format, keep, printer);
    }
    let meta = pick(&store, args.run.as_deref(), keep)?;
    let unreadable = |err: io::Error| {
        AyeError::NoRun(format!(
            "the log of run {} can't be read from {}: {err}",
            meta.run,
            store.dir().display()
        ))
    };
    if args.log {
        let mut out = io::BufWriter::new(printer.stdout().lock());
        return match store
            .copy_log(&meta.run.to_string(), &mut out)
            .and_then(|()| out.flush())
        {
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::InvalidData
                        | io::ErrorKind::InvalidInput
                ) =>
            {
                Err(unreadable(err))
            }
            other => Ok(other?),
        };
    }
    let records = store.log(&meta.run.to_string()).map_err(unreadable)?;
    let index = RunIndex::of(&records);
    let keys = Keys::of(&records);

    let wanted: Vec<(&str, Wanted)> = args
        .tests
        .iter()
        .map(|arg| (arg.as_str(), read(cwd, root, arg)))
        .collect();
    let details = args.failed
        || wanted
            .iter()
            .any(|(_, wanted)| matches!(wanted, Wanted::Test(_)));
    let mode = if details {
        ShowMode::Details
    } else {
        ShowMode::List
    };
    let chosen = choose(&index, &keys, &wanted, &meta)?;
    let filter = Filter::new(args)?;
    let filtered = filter.is_some() || !wanted.is_empty();
    let shown: Vec<&RunItem<'_>> = index
        .items
        .iter_enumerated()
        .filter(|(at, _)| chosen.contains(at))
        .map(|(_, item)| item)
        // A failed lifetime that kept tests from running is listed through
        // their lines, as the summary counts it through theirs.
        .filter(|item| mode == ShowMode::Details || !item.blocking)
        .filter(|item| {
            filter
                .as_ref()
                .is_none_or(|f| f.keeps(item, &index.case(&item.name)))
        })
        .collect();

    let feed = feed(&index, &shown, mode, filtered, meta.exit_code);
    let stale = stale(&meta, root, &shown);
    let context = context(&meta, &stale, now_ms());
    let verbosity = settings.level(details);
    let shown = RenderMode::Show {
        mode,
        keys,
        context,
    };
    let mut out: Box<dyn Consumer> = match format {
        OutputFormat::Text => {
            let mut text = Text::new(
                io::stdout(),
                TextOptions {
                    verbosity,
                    color: settings.color,
                    live: false,
                },
            )
            .with_mode(shown);
            for (path, change) in &stale {
                text.stale(path.as_str(), *change)?;
            }
            Box::new(text)
        }
        OutputFormat::Json => {
            let mut json = JsonLines::new(io::stdout(), verbosity).with_mode(shown);
            for (path, change) in &stale {
                json.stale(path.as_str(), *change)?;
            }
            Box::new(json)
        }
    };
    for record in &feed {
        match out.consume(record) {
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            other => other?,
        }
    }
    match out.finish() {
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => Ok(other?),
    }
}

/// No run is stored: why, when the configuration says.
fn no_run(store: &RunStore, keep: usize) -> AyeError {
    if keep == 0 {
        AyeError::NoRun(
            "no run is kept: [tool.aye] keep-runs = 0 (set it above 0 for aye show)".into(),
        )
    } else {
        AyeError::NoRun(format!(
            "no run is stored in {} yet: run aye first",
            store.dir().display()
        ))
    }
}

/// The run `--run` names ([`RunSelector`]: a run's number, `r81` or `81`,
/// or, for a run stored before runs were numbered, the start of its hex
/// id), else the last ([`RunStore::last`]: the newest stored when the one
/// `last` names is gone).
fn pick(store: &RunStore, wanted: Option<&str>, keep: usize) -> Result<RunMeta, AyeError> {
    let selector = wanted.map_or(RunSelector::Last, RunSelector::from);
    let wanted = wanted.unwrap_or_default();
    store.select(&selector).map_err(|err| match err {
        SelectRunError::NoRun => no_run(store, keep),
        SelectRunError::NoSuchRun { stored } => AyeError::usage(format!(
            "--run {wanted}: no stored run is numbered so{}; stored: {}",
            if stored.iter().any(|id| id.number().is_none()) {
                " or has an id that starts with it"
            } else {
                ""
            },
            ids(&stored)
        )),
        SelectRunError::Ambiguous { ids: found } => AyeError::usage(format!(
            "--run {wanted}: several stored runs' ids start with it: {}",
            ids(&found)
        )),
    })
}

fn ids(runs: &[StoredRunId]) -> String {
    runs.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests;
