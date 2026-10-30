//! `aye show --runs`: the stored runs, a line each, in text or JSON
//! ([`list_runs`], the JSON through [`RunLine`]), with the dates
//! aye-render's words say (`aye_render::words::{utc, rfc3339}`). The
//! stored runs are no events, so no renderer lists them: the binary's
//! printer writes them.

use std::io::{self, Write as _};
use std::time::Duration;

use aye_config::OutputFormat;
use aye_events::StopReason;
use aye_history::{RunMeta, RunStore};
use aye_render::summary_words;
use aye_render::words::{rfc3339, utc};
use aye_stream::Extra;
use serde::Serialize;

use super::no_run;
use crate::error::AyeError;
use crate::printer::Printer;

/// `aye show --runs`: the stored runs, the last first (the one `aye show`
/// shows), then the one that finished last.
pub(super) fn list_runs(
    store: &RunStore,
    format: OutputFormat,
    keep: usize,
    printer: Printer,
) -> Result<(), AyeError> {
    let runs = store.runs();
    if runs.is_empty() {
        return Err(no_run(store, keep));
    }
    let mut out = Vec::new();
    for (at, meta) in runs.iter().enumerate() {
        let is_last = at == 0;
        match format {
            OutputFormat::Json => {
                serde_json::to_writer(&mut out, &RunLine::new(meta, is_last))
                    .map_err(io::Error::from)?;
                writeln!(out)?;
            }
            OutputFormat::Text => {
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
    match printer.stdout().lock().write_all(&out) {
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => Ok(other?),
    }
}

/// A stored run as `aye show --runs --output json` lists it, each field in
/// the order it is written: `type` (`run`), `id`, `started` (UTC, RFC
/// 3339), `exit`, the non-zero counts as the summary names them (`skipped`
/// with the modules skipped while collecting), then `collection_error`,
/// `usage_error` and `internal_error` (error diagnostics, by kind, as the
/// text summary counts them), `ms`, `stopped` and `last` (only when true).
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename = "run")]
pub(super) struct RunLine {
    id: String,
    started: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit: Option<u8>,
    #[serde(skip_serializing_if = "is_zero")]
    passed: u32,
    #[serde(skip_serializing_if = "is_zero")]
    failed: u32,
    #[serde(skip_serializing_if = "is_zero")]
    error: u32,
    #[serde(skip_serializing_if = "is_zero")]
    skipped: u32,
    #[serde(skip_serializing_if = "is_zero")]
    expected_failure: u32,
    #[serde(skip_serializing_if = "is_zero")]
    unexpected_pass: u32,
    #[serde(skip_serializing_if = "is_zero")]
    deselected: u32,
    #[serde(skip_serializing_if = "is_zero")]
    setup_error: u32,
    #[serde(skip_serializing_if = "is_zero")]
    teardown_error: u32,
    #[serde(skip_serializing_if = "is_zero")]
    lifetime_error: u32,
    #[serde(skip_serializing_if = "is_zero")]
    collection_error: u32,
    #[serde(skip_serializing_if = "is_zero")]
    usage_error: u32,
    #[serde(skip_serializing_if = "is_zero")]
    internal_error: u32,
    ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped: Option<StopReason>,
    #[serde(skip_serializing_if = "is_false")]
    last: bool,
}

impl RunLine {
    /// The line of the stored run `meta`, the `last` one when it is.
    pub(super) fn new(meta: &RunMeta, last: bool) -> Self {
        let counts = &meta.counts;
        let extra = Extra::from_named(&meta.extra);
        Self {
            id: meta.run.to_string(),
            started: rfc3339(meta.started_ms),
            exit: meta.exit_code.map(u8::from),
            passed: counts.passed,
            failed: counts.failed,
            error: counts.error,
            skipped: counts.skipped + extra.skipped_modules,
            expected_failure: counts.expected_failure,
            unexpected_pass: counts.unexpected_pass,
            deselected: counts.deselected,
            setup_error: extra.setup_error,
            teardown_error: extra.teardown_error,
            lifetime_error: extra.lifetime_error,
            collection_error: extra.collection_error,
            usage_error: extra.usage_error,
            internal_error: extra.internal_error,
            ms: meta.ms,
            stopped: meta.stopped.clone(),
            last,
        }
    }
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "signature fixed by serde"
)]
const fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "signature fixed by serde"
)]
const fn is_false(b: &bool) -> bool {
    !*b
}
