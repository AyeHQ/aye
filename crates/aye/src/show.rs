//! `aye show`: a stored run's results again, without running anything.
//!
//! It reads the run store (`.aye/runs`, `aye_history::runs`) and feeds the
//! renderers part of a stored log, with the whole log's test keys
//! ([`Keys::of`]), so that what they write reads as the run wrote it, and
//! what it knows of the run beside the log ([`ShowContext`]: its number,
//! how long ago it finished, which of the files shown changed since).
//! What it shows are the log's results ([`Item`]): each test's result, and
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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aye_config::AyeConfig;
use aye_events::{
    Case, Certainty, Counts, Diagnostic, Event, ExitCode, LifetimeId, Outcome, Record, Severity,
    TestId, TestResult,
};
use aye_history::{Change, RunMeta, RunStore};
use aye_render::{
    ColorChoice, Extra, JsonLines, Keys, Reporter, ShowContext, ShowMode, Text, TextOptions,
    summary_words,
};
use aye_select::Selection;

use crate::args::{Color, Output, ShowArgs};
use crate::session;

/// How many of the closest names a selector naming nothing lists.
const CLOSEST: usize = 5;

/// Why `aye show` shows nothing.
enum Problem {
    /// What was asked can't be shown (exit 4).
    Usage(String),
    /// No run is stored (exit 5).
    NoRun(String),
}

impl From<io::Error> for Problem {
    fn from(err: io::Error) -> Self {
        Self::Usage(format!("cannot write output: {err}"))
    }
}

#[expect(
    clippy::print_stderr,
    reason = "aye's own messages, outside the event stream"
)]
pub(crate) fn main(args: &ShowArgs) -> ExitCode {
    match show(args) {
        Ok(()) => ExitCode::Ok,
        Err(Problem::Usage(message)) => {
            eprintln!("aye: error: {message}");
            ExitCode::UsageError
        }
        Err(Problem::NoRun(message)) => {
            eprintln!("aye: {message}");
            ExitCode::NoTests
        }
    }
}

/// A positional argument, as `aye show` reads it.
#[derive(Debug, Clone)]
enum Wanted {
    /// A file or directory, relative to the root: alone, it filters the
    /// list; beside the others, it selects its results.
    Path(String),
    /// A key or its start, an id or a dotted name.
    Test(String),
}

fn show(args: &ShowArgs) -> Result<(), Problem> {
    args.check().map_err(Problem::Usage)?;
    let cwd = std::env::current_dir()
        .map_err(|err| Problem::Usage(format!("cannot read the current directory: {err}")))?;
    let given: Vec<PathBuf> = args
        .tests
        .iter()
        .map(|test| session::path_part(test))
        .filter(|path| session::is_path_like(&cwd, path))
        .map(|path| aye_fs::normalize_absolute(&cwd.join(path)))
        .collect();
    let root = aye_fs::normalize_absolute(&aye_config::find_root(&aye_fs::common_ancestor(
        &cwd, &given,
    )));
    let config = AyeConfig::load(&root).map_err(|err| Problem::Usage(err.to_string()))?;
    let format = args
        .output
        .or_else(|| config.settings.output.as_deref()?.parse().ok())
        .unwrap_or(Output::Text);
    let store = RunStore::new(aye_fs::StateDir::for_root(&root).bucket(aye_fs::StateBucket::Runs));
    let keep = config.settings.keep_runs();
    if args.runs {
        return list_runs(&store, format, keep);
    }
    let meta = pick(&store, args.run.as_deref(), keep)?;
    let unreadable = |err: io::Error| {
        Problem::NoRun(format!(
            "the log of run {} can't be read from {}: {err}",
            meta.run,
            store.dir().display()
        ))
    };
    if args.log {
        let mut out = io::BufWriter::new(io::stdout().lock());
        return match store
            .copy_log(&meta.run, &mut out)
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
    let records = store.log(&meta.run).map_err(unreadable)?;
    let log = Log::of(&records);
    let keys = Keys::of(&records);

    let wanted: Vec<(&str, Wanted)> = args
        .tests
        .iter()
        .map(|arg| (arg.as_str(), read(&cwd, &root, arg)))
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
    let chosen = choose(&log, &keys, &wanted, &meta)?;
    let filter = Filter::new(args)?;
    let filtered = filter.is_some() || !wanted.is_empty();
    let shown: Vec<&Item<'_>> = log
        .items
        .iter()
        .enumerate()
        .filter(|(at, _)| chosen.contains(at))
        .map(|(_, item)| item)
        // A failed lifetime that kept tests from running is listed through
        // their lines, as the summary counts it through theirs.
        .filter(|item| mode == ShowMode::Details || !item.blocking)
        .filter(|item| {
            filter
                .as_ref()
                .is_none_or(|f| f.keeps(item, &log.case(&item.name)))
        })
        .collect();

    let feed = feed(&records, &log, &shown, mode, filtered, meta.exit_code);
    let stale = stale(&meta, &root, &shown);
    let context = context(&meta, &stale, now_ms());
    let verbosity = args.verbosity(details, format);
    let mut out: Box<dyn Reporter> = match format {
        Output::Text => {
            let color = match args.color {
                Color::Auto => ColorChoice::Auto,
                Color::Always => ColorChoice::Always,
                Color::Never => ColorChoice::Never,
            };
            let mut text = Text::new(
                io::stdout(),
                TextOptions {
                    verbosity,
                    color,
                    live: false,
                },
            )
            .for_show(mode, keys, context);
            for (path, change) in &stale {
                text.stale(path, *change == Change::Gone)?;
            }
            Box::new(text)
        }
        Output::Json => {
            let mut json = JsonLines::new(io::stdout(), verbosity).for_show(mode, keys, context);
            for (path, change) in &stale {
                json.stale(path, *change == Change::Gone)?;
            }
            Box::new(json)
        }
    };
    for record in &feed {
        match out.report(record) {
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
fn no_run(store: &RunStore, keep: usize) -> Problem {
    if keep == 0 {
        Problem::NoRun(
            "no run is kept: [tool.aye] keep-runs = 0 (set it above 0 for aye show)".into(),
        )
    } else {
        Problem::NoRun(format!(
            "no run is stored in {} yet: run aye first",
            store.dir().display()
        ))
    }
}

/// The run `--run` names, else the last ([`RunStore::last`]: the newest
/// stored when the one `last` names is gone). `--run` takes a run's number
/// (`r81` or `81`) or, for a run stored before runs were numbered, the
/// start of its hex id.
fn pick(store: &RunStore, wanted: Option<&str>, keep: usize) -> Result<RunMeta, Problem> {
    let Some(wanted) = wanted else {
        return store.last().ok_or_else(|| no_run(store, keep));
    };
    let runs = store.runs();
    if runs.is_empty() {
        return Err(no_run(store, keep));
    }
    let lower = wanted.to_ascii_lowercase();
    let number = aye_history::runs::number(&lower)
        .or_else(|| aye_history::runs::number(&format!("r{lower}")));
    if let Some(number) = number
        && let Some(meta) = runs.iter().find(|meta| meta.number() == Some(number))
    {
        return Ok(meta.clone());
    }
    let mut found: Vec<RunMeta> = runs
        .iter()
        .filter(|meta| meta.number().is_none() && meta.run.starts_with(&lower))
        .cloned()
        .collect();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(Problem::Usage(format!(
            "--run {wanted}: no stored run is numbered so{}; stored: {}",
            if runs.iter().any(|meta| meta.number().is_none()) {
                " or has an id that starts with it"
            } else {
                ""
            },
            ids(&runs)
        ))),
        _ => Err(Problem::Usage(format!(
            "--run {wanted}: several stored runs' ids start with it: {}",
            ids(&found)
        ))),
    }
}

fn ids(runs: &[RunMeta]) -> String {
    runs.iter()
        .map(|meta| meta.run.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// `aye show --runs`: the stored runs, the last first (the one `aye show`
/// shows), then the one that finished last.
fn list_runs(store: &RunStore, format: Output, keep: usize) -> Result<(), Problem> {
    let runs = store.runs();
    if runs.is_empty() {
        return Err(no_run(store, keep));
    }
    let mut out = Vec::new();
    for (at, meta) in runs.iter().enumerate() {
        let is_last = at == 0;
        match format {
            Output::Json => writeln!(out, "{}", run_line(meta, is_last))?,
            Output::Text => {
                let exit = meta.exit_code.map_or_else(
                    || "exit ?".to_owned(),
                    |code| format!("exit {}", u8::from(code)),
                );
                let words = summary_words(&meta.counts, &Extra::from_named(&meta.extra));
                let counts = if words.is_empty() {
                    "no tests ran".to_owned()
                } else {
                    words.join(", ")
                };
                let seconds = Duration::from_millis(meta.ms).as_secs_f64();
                let last = if is_last { "  (last)" } else { "" };
                writeln!(
                    out,
                    "{}  {}  {exit}  {counts} in {seconds:.2}s{last}",
                    meta.run,
                    utc(meta.started_ms)
                )?;
            }
        }
    }
    match io::stdout().lock().write_all(&out) {
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => Ok(other?),
    }
}

/// A stored run as `aye show --runs --output json` lists it: `type`
/// (`run`), `id`, `started` (UTC, RFC 3339), `exit`, the non-zero counts
/// as the summary names them, then `collection_error`, `usage_error` and
/// `internal_error` (error diagnostics, by kind, as the text summary counts
/// them), `ms`, `stopped` and `last` (only when true), in that order.
fn run_line(meta: &RunMeta, last: bool) -> String {
    let string = |text: &str| serde_json::Value::from(text).to_string();
    let mut fields = vec![
        r#""type":"run""#.to_owned(),
        format!(r#""id":{}"#, string(&meta.run)),
        format!(r#""started":{}"#, string(&rfc3339(meta.started_ms))),
    ];
    if let Some(code) = meta.exit_code {
        fields.push(format!(r#""exit":{}"#, u8::from(code)));
    }
    for (name, n) in counts(&meta.counts, &Extra::from_named(&meta.extra)) {
        fields.push(format!(r#""{name}":{n}"#));
    }
    fields.push(format!(r#""ms":{}"#, meta.ms));
    if let Some(stopped) = &meta.stopped {
        fields.push(format!(r#""stopped":{}"#, string(stopped)));
    }
    if last {
        fields.push(r#""last":true"#.to_owned());
    }
    format!("{{{}}}", fields.join(","))
}

/// The non-zero counts, by their names in the JSON summary (`skipped` with
/// the modules skipped while collecting), then the error diagnostics'.
fn counts(counts: &Counts, extra: &Extra) -> Vec<(&'static str, u32)> {
    [
        ("passed", counts.passed),
        ("failed", counts.failed),
        ("error", counts.error),
        ("skipped", counts.skipped + extra.skipped_modules),
        ("expected_failure", counts.expected_failure),
        ("unexpected_pass", counts.unexpected_pass),
        ("deselected", counts.deselected),
        ("setup_error", extra.setup_error),
        ("teardown_error", extra.teardown_error),
        ("lifetime_error", extra.lifetime_error),
        ("collection_error", extra.collection_error),
        ("usage_error", extra.usage_error),
        ("internal_error", extra.internal_error),
    ]
    .into_iter()
    .filter(|(_, n)| *n > 0)
    .collect()
}

/// How `aye show` reads a positional argument: a path (read as a run reads
/// one), or a test's key or its start, id or dotted name.
fn read(cwd: &Path, root: &Path, arg: &str) -> Wanted {
    let positional = session::resolve(cwd, root, arg);
    match positional.path {
        Some(_) if !positional.id => Wanted::Path(positional.selector),
        _ => Wanted::Test(positional.selector),
    }
}

/// The results the arguments select, by their place in [`Log::items`]:
/// every one without any; for a key, or the start of one test's or
/// lifetime's key ([`Keys::resolve`]), that test's every result or that
/// lifetime; for another selector, every result it names (a test, a
/// lifetime, a file's diagnostics, as an id names them). One that names
/// nothing of the run is a usage error, listing the closest names; the
/// start of several keys too, listing them.
fn choose(
    log: &Log<'_>,
    keys: &Keys,
    wanted: &[(&str, Wanted)],
    meta: &RunMeta,
) -> Result<HashSet<usize>, Problem> {
    if wanted.is_empty() {
        return Ok((0..log.items.len()).collect());
    }
    let mut chosen = HashSet::new();
    let mut problems = Vec::new();
    let names = log.names();
    let with_results: HashSet<&str> = names.iter().map(TestId::as_str).collect();
    let mut selectors: Vec<(&str, &str)> = Vec::new();
    for (arg, wanted) in wanted {
        let selector = match wanted {
            Wanted::Path(selector) => selector,
            Wanted::Test(selector) => {
                let found = keys.resolve(arg);
                if found.is_empty() {
                    selector
                } else {
                    match by_key(arg, &found, &with_results, keys, &meta.run) {
                        Ok(id) => chosen.extend(
                            log.items
                                .iter()
                                .enumerate()
                                .filter(|(_, item)| item.name.as_str() == id)
                                .map(|(at, _)| at),
                        ),
                        Err(problem) => problems.push(problem),
                    }
                    continue;
                }
            }
        };
        selectors.push((arg, selector.as_str()));
    }
    if !selectors.is_empty() {
        let selection = Selection::default().with_ids(selectors.iter().map(|(_, s)| *s));
        let selected = selection.apply(names.iter().map(|name| log.case(name)).collect());
        let kept: HashSet<TestId> = selected.cases.into_iter().map(|case| case.id).collect();
        chosen.extend(
            log.items
                .iter()
                .enumerate()
                .filter(|(_, item)| kept.contains(&item.name))
                .map(|(at, _)| at),
        );
        for unmatched in &selected.unmatched {
            let arg = selectors
                .iter()
                .find(|(_, selector)| selector == unmatched)
                .map_or(unmatched.as_str(), |(arg, _)| *arg);
            problems.push(nothing_named(arg, &meta.run, &closest(arg, &names)));
        }
    }
    if problems.is_empty() {
        Ok(chosen)
    } else {
        Err(Problem::Usage(problems.join("\naye: error: ")))
    }
}

/// The one id with a result in run `run` that `arg` names as a key, of the
/// ids `found` whose key it is or starts (those in `with_results` have one);
/// else why not: the start of several keys, or of keys none of whose tests
/// has a result.
fn by_key<'a>(
    arg: &str,
    found: &'a [String],
    with_results: &HashSet<&str>,
    keys: &Keys,
    run: &str,
) -> Result<&'a str, String> {
    let shown: Vec<&str> = found
        .iter()
        .map(String::as_str)
        .filter(|id| with_results.contains(id))
        .collect();
    match shown[..] {
        [id] => Ok(id),
        [] => Err(match found {
            [id] => format!("{arg}: the key of {id}, which has no result in run {run}"),
            _ => format!(
                "{arg}: the start of {} tests' keys, none of which has a result in run {run}",
                found.len()
            ),
        }),
        _ => {
            let key = |id: &str| keys.key(id).unwrap_or(arg);
            let width = shown.iter().map(|id| key(id).len()).max().unwrap_or(0);
            let candidates: Vec<String> = shown
                .iter()
                .map(|id| format!("\n  {:<width$}  {id}", key(id)))
                .collect();
            Err(format!(
                "{arg}: the start of several keys; give more of the one you want:{}",
                candidates.concat()
            ))
        }
    }
}

fn nothing_named(arg: &str, run: &str, closest: &[&str]) -> String {
    let mut message = format!("{arg}: no test of run {run} has that name");
    if !closest.is_empty() {
        message.push_str("; the closest:");
        for id in closest {
            message.push_str("\n  ");
            message.push_str(id);
        }
    }
    message
}

/// The names closest to `arg`, without their parameters (each selects all
/// of them), each once: by edit distance to the whole name, or between
/// the last names (a test with that name elsewhere: an argument's last
/// `::` part, else its last dotted part, else itself, against a name's
/// last `::` part or its file's name), whichever is closest.
fn closest<'a>(arg: &str, names: &'a [TestId]) -> Vec<&'a str> {
    let whole: Vec<char> = arg.chars().collect();
    // What the argument's last name may be: `tests/t.py::T::test_x` ends
    // in `test_x`, a dotted `tests.t.test_x` too, while a file's name
    // (`usage.rst`, `test_t.py`) is its own.
    let mut lasts: Vec<Vec<char>> = vec![whole.clone()];
    match arg.rsplit_once("::") {
        Some((_, name)) => lasts.push(name.chars().collect()),
        None if !arg.contains('/') => {
            if let Some((_, name)) = arg.rsplit_once('.') {
                lasts.push(name.chars().collect());
            }
        }
        None => {
            if let Some((_, name)) = arg.rsplit_once('/') {
                lasts.push(name.chars().collect());
            }
        }
    }
    let mut seen = HashSet::new();
    let mut scored: Vec<(usize, &str)> = names
        .iter()
        .map(TestId::without_params)
        .filter(|name| seen.insert(*name))
        .map(|name| {
            let path = name.split_once("::").map_or(name, |(path, _)| path);
            let file = path.rsplit_once('/').map_or(path, |(_, file)| file);
            let last = name.rsplit_once("::").map_or(file, |(_, last)| last);
            let score = lasts
                .iter()
                .flat_map(|arg| [distance(arg, last), distance(arg, file)])
                .fold(distance(&whole, name), usize::min);
            (score, name)
        })
        .collect();
    scored.sort_unstable();
    scored.into_iter().take(CLOSEST).map(|(_, id)| id).collect()
}

/// Levenshtein's distance between `a` and `b`, in characters.
fn distance(a: &[char], b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(ca != cb));
            diagonal = above;
        }
    }
    row[b.len()]
}

/// `--failed`, `--outcome`, `-k` and `-m`.
struct Filter {
    /// The outcomes kept; empty keeps any.
    outcomes: HashSet<Outcome>,
    selection: Selection,
}

impl Filter {
    fn new(args: &ShowArgs) -> Result<Option<Self>, Problem> {
        let mut outcomes: HashSet<Outcome> = args.outcome.iter().copied().collect();
        if args.failed {
            outcomes.extend([Outcome::Failed, Outcome::Error]);
        }
        let mut selection = Selection::default();
        if let Some(expr) = &args.keyword {
            selection = selection
                .with_keyword(expr)
                .map_err(|err| Problem::Usage(err.to_string()))?;
        }
        if let Some(expr) = &args.tags {
            selection = selection
                .with_tags(expr)
                .map_err(|err| Problem::Usage(err.to_string()))?;
        }
        Ok(
            (!outcomes.is_empty() || !selection.is_empty()).then_some(Self {
                outcomes,
                selection,
            }),
        )
    }

    /// Whether a result is kept: by its outcome, and by `-k` and `-m` on
    /// its case (what can't be decided is kept, as a run keeps it). One
    /// without a name (a failed worker, a diagnostic about no file) has
    /// nothing for `-k` or `-m` to match.
    fn keeps(&self, item: &Item<'_>, case: &Case) -> bool {
        (self.outcomes.is_empty() || self.outcomes.contains(&item.outcome))
            && (self.selection.is_empty()
                || (!item.name.as_str().is_empty() && !self.selection.truth(case).is_false()))
    }
}

/// A result of the run, as `aye show` lists and selects them.
struct Item<'a> {
    /// Its record's place in the log.
    at: usize,
    what: What<'a>,
    /// What selectors match: a test's id, a lifetime's, a diagnostic's
    /// path; empty for a failed worker.
    name: TestId,
    /// `error` for a failed lifetime, an error diagnostic or a failed
    /// worker; `skipped` for a module skipped while collecting.
    outcome: Outcome,
    /// A failed lifetime some test names as its `cause`.
    blocking: bool,
}

enum What<'a> {
    /// A test's result, with the places of its `test_started` and
    /// `sub_outcome` records.
    Test(&'a TestResult, Vec<usize>),
    /// A failed lifetime: the first of its failures (a class split across
    /// batches tears down in each).
    Lifetime(&'a LifetimeId),
    /// An error diagnostic (each once), or a module skipped while
    /// collecting.
    Diagnostic,
    /// A failed worker.
    Worker,
}

/// A test's records before its `test_finished`: the worker that runs it,
/// and their places in the log.
type Open = (Option<u32>, Vec<usize>);

/// What `aye show` reads of a stored log.
struct Log<'a> {
    run_started: Option<&'a Record>,
    run_finished: Option<&'a Record>,
    /// The run's results, in the order of the log.
    items: Vec<Item<'a>>,
    /// Each failed lifetime's every failure, by place in the log.
    lifetimes: HashMap<&'a LifetimeId, Vec<usize>>,
    /// The collected cases, by id.
    cases: HashMap<&'a TestId, &'a Case>,
}

impl<'a> Log<'a> {
    fn of(records: &'a [Record]) -> Self {
        let mut log = Log {
            run_started: None,
            run_finished: None,
            items: Vec::new(),
            lifetimes: HashMap::new(),
            cases: HashMap::new(),
        };
        // A test's `test_started` and `sub_outcome` records not yet
        // finished, by id and worker.
        let mut open: HashMap<&TestId, Vec<Open>> = HashMap::new();
        let mut causes: HashSet<&str> = HashSet::new();
        let mut diagnostics: HashSet<(&str, &str, &str)> = HashSet::new();
        for (at, record) in records.iter().enumerate() {
            let worker = record.envelope.worker;
            match &record.event {
                Event::RunStarted { .. } if log.run_started.is_none() => {
                    log.run_started = Some(record);
                }
                Event::RunFinished { .. } => log.run_finished = Some(record),
                Event::CaseCollected(case) => {
                    log.cases.insert(&case.id, case);
                }
                Event::TestStarted { id } | Event::SubOutcome { id, .. } => {
                    let tests = open.entry(id).or_default();
                    match tests.iter_mut().find(|(w, _)| *w == worker) {
                        Some((_, parts)) => parts.push(at),
                        None => tests.push((worker, vec![at])),
                    }
                }
                Event::TestFinished(result) => {
                    // Its own worker's, else (a test the runner failed for
                    // a worker that crashed) the first open.
                    let parts = open.get_mut(&result.id).map_or_else(Vec::new, |tests| {
                        let found = tests.iter().position(|(w, _)| *w == worker);
                        match found.or_else(|| (!tests.is_empty()).then_some(0)) {
                            Some(at) => tests.remove(at).1,
                            None => Vec::new(),
                        }
                    });
                    if let Some(cause) = &result.cause {
                        causes.insert(cause);
                    }
                    log.items.push(Item {
                        at,
                        what: What::Test(result, parts),
                        name: result.id.clone(),
                        outcome: result.outcome,
                        blocking: false,
                    });
                }
                Event::LifetimeFinished {
                    lifetime,
                    failure: Some(_),
                    ..
                } => {
                    let failures = log.lifetimes.entry(lifetime).or_default();
                    failures.push(at);
                    if failures.len() == 1 {
                        log.items.push(Item {
                            at,
                            what: What::Lifetime(lifetime),
                            name: TestId::new(lifetime.as_str()),
                            outcome: Outcome::Error,
                            blocking: false,
                        });
                    }
                }
                Event::Diagnostic(diagnostic) => {
                    let outcome = if aye_render::module_skip(record).is_some() {
                        Outcome::Skipped
                    } else if diagnostic.severity == Severity::Error {
                        Outcome::Error
                    } else {
                        continue;
                    };
                    let Diagnostic {
                        code,
                        path,
                        message,
                        ..
                    } = diagnostic;
                    // Each once, as the renderers show them; a module once.
                    let key = match outcome {
                        Outcome::Skipped => ("", path.as_str(), ""),
                        _ => (code.as_str(), path.as_str(), message.as_str()),
                    };
                    if diagnostics.insert(key) {
                        log.items.push(Item {
                            at,
                            what: What::Diagnostic,
                            name: TestId::new(path.as_str()),
                            outcome,
                            blocking: false,
                        });
                    }
                }
                Event::WorkerFinished { error: Some(_), .. } => log.items.push(Item {
                    at,
                    what: What::Worker,
                    name: TestId::new(""),
                    outcome: Outcome::Error,
                    blocking: false,
                }),
                _ => {}
            }
        }
        for item in &mut log.items {
            if let What::Lifetime(lifetime) = item.what {
                item.blocking = causes.contains(lifetime.as_str());
            }
        }
        log
    }

    /// The names selectors match, each once, in the order they come.
    fn names(&self) -> Vec<TestId> {
        let mut seen = HashSet::new();
        self.items
            .iter()
            .filter(|item| !item.name.as_str().is_empty() && seen.insert(&item.name))
            .map(|item| item.name.clone())
            .collect()
    }

    /// The case of a name, as collection listed it: its own, or that of
    /// its id without parameters (a case collected without them finishes
    /// as one result per parameter), with its id; a bare one when
    /// collection listed neither (a lifetime, a file).
    fn case(&self, id: &TestId) -> Case {
        let found = self
            .cases
            .get(id)
            .copied()
            .or_else(|| self.cases.get(&TestId::new(id.without_params())).copied());
        let mut case = found.cloned().unwrap_or_else(|| Case::new(id.as_str(), ""));
        case.id = id.clone();
        if case.certainty == Certainty::Partial {
            case.certainty = Certainty::Refined;
        }
        case
    }
}

/// The records the renderer gets: the run's `run_started`, what `mode`
/// shows of the results `shown` (in details, with the `worker_started` of
/// the workers that ran them), and a `run_finished`: the run's own when
/// the list is the whole run (nothing `filtered`), else one counting the
/// tests shown, with the run's exit code (`exit_code`, the stored one when
/// the log has no `run_finished`).
fn feed(
    records: &[Record],
    log: &Log<'_>,
    shown: &[&Item<'_>],
    mode: ShowMode,
    filtered: bool,
    exit_code: Option<ExitCode>,
) -> Vec<Record> {
    let mut places = BTreeSet::new();
    for item in shown {
        places.insert(item.at);
        if mode == ShowMode::List {
            continue;
        }
        match &item.what {
            What::Test(result, parts) => {
                places.extend(parts);
                if let Some(cause) = &result.cause {
                    places.extend(log.lifetimes.get(cause).into_iter().flatten());
                }
            }
            What::Lifetime(lifetime) => {
                places.extend(log.lifetimes.get(lifetime).into_iter().flatten());
            }
            What::Diagnostic | What::Worker => {}
        }
    }
    if mode == ShowMode::Details {
        // The `worker_started` of each worker that ran a result shown: it
        // says which Python ran it.
        let workers: HashSet<u32> = places
            .iter()
            .filter_map(|at| records[*at].envelope.worker)
            .collect();
        places.extend(records.iter().enumerate().filter_map(|(at, record)| {
            (matches!(record.event, Event::WorkerStarted { .. })
                && record
                    .envelope
                    .worker
                    .is_some_and(|worker| workers.contains(&worker)))
            .then_some(at)
        }));
    }
    let mut feed: Vec<Record> = log.run_started.into_iter().cloned().collect();
    feed.extend(places.into_iter().map(|at| records[at].clone()));
    let whole = mode == ShowMode::List && !filtered;
    match log.run_finished {
        Some(finished) if whole => feed.push(finished.clone()),
        _ => {
            let mut counts = Counts::default();
            let mut duration = Duration::ZERO;
            for item in shown {
                if let What::Test(result, _) = &item.what {
                    counts.record(result.outcome);
                    duration = duration.saturating_add(result.duration);
                }
            }
            let exit_code = log
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
            record.envelope.run = log
                .run_started
                .and_then(|started| started.envelope.run.clone());
            feed.push(record);
        }
    }
    feed
}

/// The files of the results shown that changed since the run, each once,
/// in the order their results come.
fn stale(meta: &RunMeta, root: &Path, shown: &[&Item<'_>]) -> Vec<(String, Change)> {
    let mut seen = HashSet::new();
    shown
        .iter()
        .map(|item| item.name.path())
        .filter(|path| !path.is_empty() && seen.insert(*path))
        .filter_map(|path| Some((path.to_owned(), meta.change(root, path)?)))
        .collect()
}

/// What the renderers are told of the run `meta` beside its log, at
/// `now_ms`: its number, how long ago it finished, and the files of the
/// results shown that changed since (`stale`), each with whether it is
/// gone.
fn context(meta: &RunMeta, stale: &[(String, Change)], now_ms: u64) -> ShowContext {
    ShowContext {
        run: meta.number(),
        ago: (meta.finished_ms > 0).then(|| ago(now_ms, meta.finished_ms)),
        stale: stale
            .iter()
            .map(|(path, change)| (path.clone(), *change == Change::Gone))
            .collect::<BTreeMap<_, _>>(),
    }
}

/// How long before `now_ms` the time `then_ms` was (both in milliseconds
/// since the Unix epoch): `just now` (under a minute, or later than now),
/// `2 min ago`, `3 h ago`, `1 day ago`, `2 days ago`.
fn ago(now_ms: u64, then_ms: u64) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        ..60 => "just now".to_owned(),
        60..3_600 => format!("{} min ago", secs / 60),
        3_600..86_400 => format!("{} h ago", secs / 3_600),
        86_400..172_800 => "1 day ago".to_owned(),
        _ => format!("{} days ago", secs / 86_400),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// `2026-10-15 12:34:56 UTC`.
fn utc(ms: u64) -> String {
    let (date, time) = civil(ms);
    format!("{date} {time} UTC")
}

/// `2026-10-15T12:34:56Z`.
fn rfc3339(ms: u64) -> String {
    let (date, time) = civil(ms);
    format!("{date}T{time}Z")
}

/// The date and time of `ms` milliseconds after the Unix epoch, in UTC:
/// `2026-10-15` and `12:34:56` (Howard Hinnant's `civil_from_days`).
fn civil(ms: u64) -> (String, String) {
    let secs = ms / 1000;
    let days = i64::try_from(secs / 86_400).unwrap_or(i64::MAX / 2);
    let of_day = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        format!("{year:04}-{month:02}-{day:02}"),
        format!(
            "{:02}:{:02}:{:02}",
            of_day / 3600,
            of_day % 3600 / 60,
            of_day % 60
        ),
    )
}

#[cfg(test)]
mod tests;
