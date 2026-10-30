//! `aye show --runs`: the stored runs, a line each, in text or JSON
//! ([`list_runs`]), with the dates aye-render's words say
//! (`aye_render::words::{utc, rfc3339}`).

use std::io::{self, Write as _};
use std::time::Duration;

use aye_config::OutputFormat;
use aye_events::Counts;
use aye_history::{RunMeta, RunStore};
use aye_render::summary_words;
use aye_render::words::{rfc3339, utc};
use aye_stream::Extra;

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
            OutputFormat::Json => writeln!(out, "{}", run_line(meta, is_last))?,
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

/// A stored run as `aye show --runs --output json` lists it: `type`
/// (`run`), `id`, `started` (UTC, RFC 3339), `exit`, the non-zero counts
/// as the summary names them, then `collection_error`, `usage_error` and
/// `internal_error` (error diagnostics, by kind, as the text summary counts
/// them), `ms`, `stopped` and `last` (only when true), in that order.
pub(super) fn run_line(meta: &RunMeta, last: bool) -> String {
    let string = |text: &str| serde_json::Value::from(text).to_string();
    let mut fields = vec![
        r#""type":"run""#.to_owned(),
        format!(r#""id":{}"#, string(&meta.run.to_string())),
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
        fields.push(format!(r#""stopped":{}"#, string(stopped.as_str())));
    }
    if last {
        fields.push(r#""last":true"#.to_owned());
    }
    format!("{{{}}}", fields.join(","))
}

/// The non-zero counts, by their names in the JSON summary (`skipped` with
/// the modules skipped while collecting), then the error diagnostics'.
pub(super) fn counts(counts: &Counts, extra: &Extra) -> Vec<(&'static str, u32)> {
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
