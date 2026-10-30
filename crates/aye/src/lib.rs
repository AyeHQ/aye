//! The `aye` binary, as a library: [`main`] reads the command line
//! (`cli.rs`: which command, and its arguments as clap reads them), the
//! environment aye started in, once (`environment.rs`), resolves the
//! command's settings from them and `[tool.aye]`, once and in the order that
//! decides which usage error wins (`settings.rs`), and runs the command
//! (`commands.rs`): a run (`commands/run.rs`, whose exit code
//! `commands/run/exit.rs` decides), `aye config` (`commands/config.rs`) or
//! `aye show` (`commands/show.rs`). What ends aye before a run's
//! event stream starts is an `AyeError` (`error.rs`), and everything the
//! binary writes beside the stream goes through its `Printer`
//! (`printer.rs`). `main.rs` only calls [`main`].
//!
//! Holds the static list of adapters the runner knows about, and which of
//! them a project runs (`registry.rs`); orchestration lives in `aye-runner`,
//! presentation in `aye-render`. Linking an adapter here is the only place
//! an adapter is depended on.
//!
//! Depends on: every runner crate and every `aye-adapter-*` crate.
//!
//! Decided (implementation, October 2026), beside what each module says of
//! its own part:
//! - **No whole-home runs.** With no path, a run from the home directory or
//!   a filesystem root is a usage error before anything walks (a home
//!   directory under git is a project root, and pytest would collect every
//!   project in it); a path given (`aye .`) runs.
//! - **Never silent, always stoppable.** While the run's settings resolve
//!   (preparing the pytest adapter may walk a big tree), a `collecting…`
//!   line shows on a terminal after half a second (the printer's `Busy`).
//!   Collection then sends `collection_progress` (after half a second, at
//!   most four a second).
//!   The first Ctrl-C before the run stops collection, which ends as
//!   interrupted (exit 2); if what is running then isn't watching, aye
//!   exits 2 after a second and a half anyway, its workers killed.
//! - **The arguments after `--`** are the framework's: aye's pytest
//!   front-end parses them after `addopts` and `PYTEST_ADDOPTS`, as pytest
//!   would, so those that replace a test run (pytest's `-h`, `-V`, `--co`,
//!   `--fixtures`, `--fixtures-per-test`, `--markers`, also inside a cluster
//!   such as `-vh`) are usage errors naming the aye equivalent, before
//!   anything runs. So are `--setup-plan` and `--setup-only`: they run no
//!   test, and every test would be reported, and remembered for `--lf`, as
//!   skipped. `--setup-show` runs the tests and is allowed.
//! - **Which adapters run** is `[tool.aye] adapters`, in its order (an
//!   unknown or repeated name, or an empty list, is a usage error). Unset,
//!   it is doctest then pytest, for every project and whatever the
//!   interpreter installs (October 2026; it used to fall back to unittest
//!   where pytest wasn't installed): aye's engine runs pytest's tests
//!   without pytest, pytest's rules run `TestCase` classes too, pytest
//!   lists a file's doctests before its other tests, and the doctest
//!   adapter prepares nothing unless doctests are on. A project that wants
//!   `python -m unittest discover`'s rules lists `unittest`. Each adapter
//!   prepares with `prepare_among`, knowing the others. `aye config` shows
//!   the adapters and why. The table of an adapter that doesn't run
//!   (`[tool.aye.unittest]`) is still checked: its bad values are usage
//!   errors too.
//! - **The framework's own `-k` and `-m`** (pytest's from `addopts` and
//!   `PYTEST_ADDOPTS`, in its adapter's `FrameworkDefaults`) apply
//!   statically to that adapter's cases when the command line gives none,
//!   so `--co` and `deselected` agree with pytest; other adapters' cases
//!   are left alone unless their frameworks name their own. One after `--` wins over both,
//!   for every case, since those arguments come last in pytest's. Where
//!   they can't be read (an `@file`, an unknown short option that might hold
//!   the letter), every adapter's cases get the command line's, else their
//!   own: no adapter applies an expression at runtime, so static selection
//!   is the last word on every case. The command line's must parse.
//! - **The framework's own `--deselect`** (pytest's from `addopts`,
//!   `PYTEST_ADDOPTS` or after `--`, as id prefixes in its adapter's
//!   `FrameworkDefaults`) drops that adapter's cases statically and counts
//!   them as deselected, as pytest's summary does; the adapter no longer
//!   drops them itself, so they are dropped once.
//! - **Positional paths** are relative to the current directory, with `.`
//!   and `..` resolved lexically before the project root is searched for. A
//!   path only the root has is taken from the root, so the ids aye prints
//!   rerun from any directory. One spelled through another name of a
//!   directory in the project (macOS's `/tmp` is `/private/tmp`) is spelled
//!   from the root, as collection's ids are (`aye_fs::relative_to`).
//!   Paths mixed with dotted names or keys are collected along with the
//!   adapters' default roots. Cases are listed in the order of the arguments
//!   that select them, as pytest orders them (selection and collection do
//!   it); the runner still keeps a file's cases together in a batch.
//! - **Dotted names and keys stand for the files they select.** They are
//!   resolved among what collection finds over the whole project, whose
//!   refinement starts each session from the framework's default (the
//!   adapters prepared seeing only the path arguments). So the run is the
//!   user's run of `pytest <the path arguments and those files>`: every
//!   adapter's context of a batch gets `selected_paths` (the path
//!   arguments, an id's file, and the files of the cases each other
//!   argument selects, absolute, in the order of the arguments), which the
//!   pytest and doctest adapters start their sessions from (start-up
//!   conftests, initial paths); and of collection's diagnostics, a
//!   `skipped` info (a module or directory skipped while collecting, which
//!   the summaries count as a skip) is the run's only where a path argument
//!   holds it or lies in it, or a dotted name names it, its package or
//!   something in it (`pytest --pyargs`: a dotted name naming a skipped
//!   module is found, and no `not-found`), and a `usage-error` on a file (a
//!   conftest refinement's session loaded at start-up) only where a path
//!   argument holds it, its directory holds a path the run starts from, or
//!   some argument named nothing (it may be why). The others are left out
//!   of the stream. With no positional argument, or only paths, collection
//!   walks only what the run reaches, and nothing changes.
//! - **What names nothing** (a missing path; an id, dotted name or key that
//!   matches no test) is a `not-found` error diagnostic in the stream, and
//!   the run exits 4 without running, as pytest's `not found:` does. An
//!   existing file or directory without tests names nothing to run but is
//!   no error: alone, the run exits 5.
//! - **Ids with parameters on the command line** (`path::test[x]`) go to
//!   every adapter's context as `requested_ids`, relative to the root.
//!   Where collection only knows the test without its parameters
//!   (`--no-refine`, a failed refinement), the batch gets the typed id, and
//!   the framework reports it `not-found` when it makes no such test rather
//!   than run the test's other parameters in its place, as it may for ids
//!   aye worked out itself.
//! - **A file's collection errors count as pytest counts them.** pytest
//!   collects a file's `TestCase` classes and its own tests in one
//!   `Module`, so the unittest and pytest adapters' batches that both load
//!   a file failing to import are one collection error: the file shows as
//!   many as the adapter reporting the most for it, and another adapter's
//!   beyond those become infos saying so, kept in the stream
//!   (`registry::collects_in_one_module`). The doctest adapter's are
//!   pytest's `DoctestModule`, another collector, and count apart. A
//!   conftest pytest fails to load is one error too, however many adapters
//!   load it (the doctest adapter imports the conftests pytest would): a
//!   `usage-error` or `collection-error` on a `conftest.py` that another
//!   adapter (or refinement) already reported with that code becomes an
//!   info saying so, with its own words when they differ. Files below a
//!   conftest that failed to load have no tests because of it, which its
//!   error says: collection's `no-tests` notes on them are left out.
//! - **Reruns name the test from where aye ran.** Ids are relative to the
//!   root and a path is read from the current directory first, so from a
//!   subdirectory holding a path of the same name (`tests/tests/test_a.py`
//!   under `tests/`) an id would name the other file: there a failed
//!   test's `reproduce` command names its file relative to the current
//!   directory (one is added for an adapter that sets none), so what aye
//!   prints reruns it. Elsewhere the command is the id.
//! - **Reruns repeat aye's options that shape the run.** `-c FILE` decides
//!   which tests exist and how they run, so every adapter's context of a
//!   batch gets `rerun_args` (`["-c", FILE]`, as given, for the directory
//!   aye ran in), which adapters put before the id: `aye -c FILE <id> --
//!   <args>`. `--python` is left out (a rerun finds the interpreter as aye
//!   did), and so are the options that shape only the output or the
//!   selection, which a rerun of one id doesn't need, and `--junitxml`,
//!   whose report it would overwrite.
//! - **Output.** A stdout reporter that fails (a closed pipe) stops writing,
//!   and the JUnit report and history still see every record. With `-s`,
//!   workers' output goes to stdout beside text output and to stderr beside
//!   `--output json`, so JSON lines stay whole. GitHub annotations go
//!   beside text output, at every verbosity, never beside JSON. The
//!   JUnit report goes to `--junitxml` (with `$VARIABLES` and `~` expanded,
//!   as pytest does), else to the framework's own: the `junitxml` path its
//!   adapter read from pytest's `addopts`, `PYTEST_ADDOPTS` or the
//!   arguments after `--`. Relative paths are taken from the current
//!   directory. Its `junit_prefix` (pytest's `--junitprefix`) goes before
//!   the report's classnames either way, and its JUnit settings
//!   (`junit_suite_name`, `junit_logging`, `junit_log_passing_tests`,
//!   `junit_duration_report`, `junit_family`, from the first adapter that
//!   gives each) shape the report, which is aye's own layout without them.
//!   The path is checked before anything runs (exit 4); if the report
//!   still can't be written at the end, the process exits 1 although
//!   `run_finished` said 0 or 5. GitHub annotation paths are relative to
//!   `GITHUB_WORKSPACE` (else the directory holding `.git`).
//! - **Workers and refinement.** Every worker gets the run's id as
//!   `PYTEST_XDIST_TESTRUNUID`, so pytest-xdist's `testrun_uid` is shared by
//!   a run's workers and new each run. The refinement cache is on, keyed on
//!   the interpreter workers run, the arguments after `--` and aye's own
//!   environment (which workers inherit; the run's id is left out). The
//!   plan's session is, by adapter, every file collection found tests in,
//!   selected or not, in collection order (with dotted names or keys, only
//!   those in the paths the run starts from): what each worker of an
//!   executor that lists the session (the pytest adapter's) lists before
//!   its first batch.
//! - **aye's engine runs every test** (October 2026): pytest suites run
//!   with aye's own `pytest` module and pytest is never imported. The
//!   pytest adapter's front-end refines every test file (`registry.rs`),
//!   and every adapter's workers get aye's `pytest` module, with what it
//!   stands on, before their own modules (`registry::PRELUDE`: the doctest
//!   adapter imports conftests, which import it). `aye config` says
//!   `engine   native`. `--native`, the prototype's switch, is accepted and
//!   does nothing (`AYE_NATIVE` is no longer read), and reruns leave it
//!   out. A run of pytest's tests can't do without refinement: `--no-refine`
//!   is a usage error, unless with `--co`, which lists the static
//!   recognizer's prediction without importing anything. A `refine-failed`
//!   warning is an error: the front-end is the only source of pytest's
//!   tests, so without it the run has no result rather than an empty one.
//!   A file's collection error refinement reported counts as the pytest
//!   adapter's for the one-module rule above, since its front-end lists
//!   every pytest file there.
//! - **Every run is kept for `aye show`** (October 2026), numbered (`r81`,
//!   claimed before `run_started`, which says it): the run store
//!   (`aye_history::runs`) is the stream's last consumer, serializing each
//!   record once every reporter has it and compressing the log on a thread
//!   of its own, so the run doesn't wait on it (on `packaging`'s 62,000
//!   tests, 57 MB of records, the run takes as long). Beside it the binary
//!   folds what the summaries count beside the tests (`aye_render::Tally`)
//!   and hands it to the store with the end of the stream, for `aye show
//!   --runs`. A run with nothing to show (no test ran and nothing failed
//!   but the command line's names: a typo in a path) isn't kept, so `last`
//!   stays the run before it, and its number is free again. `[tool.aye]
//!   keep-runs` (10) says how many runs are kept. A store that can't be
//!   written is a warning, never a failed run. History, folded after the
//!   reporters, first gives each bad outcome its `history` (how the test
//!   ended before this run), which reporters and the store see. `aye show`
//!   (`commands/show.rs`, detected as `aye config` is) reads it back and
//!   never runs anything: the list of a run's results (its tests, and its failed
//!   lifetimes, error diagnostics and modules skipped while collecting, so
//!   that the whole list ends with the run's own summary), or the details
//!   of the results named (by key, id, dotted name or path) or of every
//!   failure (`--failed`), through the same renderers, fed part of the log
//!   with the whole log's keys, what the store knows beside it and the
//!   run's exit code; or the stored log itself (`--log`). It needs no
//!   interpreter, and prepares no adapter. Its usage lines name it `aye
//!   show`.
//! - **Signals.** Ctrl-C, SIGTERM or SIGHUP stops the run (exit 2); a second
//!   one kills every worker, with the processes its tests started, and exits
//!   at once. A signal within half a second of the first is that one
//!   delivered again, not a second request: GNU `timeout` signals aye and
//!   then its process group, aye included, and the run must still report.

use std::ffi::OsString;
use std::process;

use aye_events::ExitCode;

mod cli;
mod commands;
mod environment;
mod error;
mod framework_args;
mod positional;
mod printer;
mod registry;
mod settings;

use cli::Command;
use commands::run::RunEnd;
use environment::EnvironmentOptions;
use error::AyeError;
use printer::Printer;
use settings::RunSettings;

/// What `aye` does with the command line `args` (the program's name
/// first), and the exit code the process ends with.
pub fn main(args: impl IntoIterator<Item = OsString>) -> process::ExitCode {
    let printer = Printer;
    run(args.into_iter().collect(), printer)
        .unwrap_or_else(|err| err.report(printer))
        .into()
}

/// The command `args` ask for, run: its arguments read, its settings
/// resolved, then the command itself. The environment is read once, here,
/// for a run or `aye config`: everything after reads what was read.
fn run(args: Vec<OsString>, printer: Printer) -> Result<ExitCode, AyeError> {
    match Command::parse(args)? {
        Command::Show(args) => {
            commands::show::show(&args, printer)?;
            Ok(ExitCode::Ok)
        }
        Command::Config(args) => {
            let settings = RunSettings::resolve(args, EnvironmentOptions::from_process(), printer)?;
            commands::config::print(&settings, printer)?;
            Ok(ExitCode::Ok)
        }
        Command::Run(args) => {
            let settings = RunSettings::resolve(args, EnvironmentOptions::from_process(), printer)?;
            Ok(process_exit_code(commands::run::run(settings, printer)?))
        }
    }
}

/// The exit code a run ends the process with: the one `run_finished`
/// carried, except that a report file that couldn't be written at the very
/// end (it was writable when the run started) fails the run, as it does
/// pytest's: 0 or 5 become 1. `run_finished` was out already, so only the
/// process can say so.
fn process_exit_code(ended: RunEnd) -> ExitCode {
    if ended.report_failed && matches!(ended.code, ExitCode::Ok | ExitCode::NoTests) {
        ExitCode::TestsFailed
    } else {
        ended.code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_that_fails_at_the_end_fails_the_process() {
        let ended = |code, report_failed| {
            process_exit_code(RunEnd {
                code,
                report_failed,
            })
        };
        assert_eq!(ended(ExitCode::Ok, true), ExitCode::TestsFailed);
        assert_eq!(ended(ExitCode::NoTests, true), ExitCode::TestsFailed);
        for code in [
            ExitCode::TestsFailed,
            ExitCode::Interrupted,
            ExitCode::InternalError,
            ExitCode::UsageError,
        ] {
            assert_eq!(ended(code, true), code);
        }
        for code in [ExitCode::Ok, ExitCode::NoTests, ExitCode::Interrupted] {
            assert_eq!(ended(code, false), code);
        }
    }
}
