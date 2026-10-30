//! L2 and L3: a bad outcome's block, its second line and its rows.

use std::io::{self, Write};

use anstream::stream::{AsLockedWrite, RawStream};
use aye_events::{Captured, Frame, Local};
use aye_stream::FileChange;

use super::Text;
use super::bad::Bad;
use super::limits::{LABEL, NOT_RUN, OUTPUT_LINES};
use super::out::Group;
use super::settings::Level;
use super::stylesheet::{DIM, KEY, RED};
use crate::{failure, words};

impl<W: RawStream + AsLockedWrite> Text<W> {
    /// L2 and L3: a block.
    pub(super) fn block_l2(&mut self, bad: &Bad<'_>, level: Level) -> io::Result<()> {
        self.block()?;
        self.head(bad.outcome, bad.key, bad.name, None, true)?;
        self.second_line(bad)?;
        let first = bad.failures.first();
        if let Some(failure) = first {
            writeln!(self.out)?;
            self.pointed_code(failure)?;
            self.explanation(failure, level)?;
            for also in &bad.failures[1..] {
                let phase = also.phase.map_or("test", words::phase);
                writeln!(
                    self.out,
                    "  {DIM}also in {phase}:{DIM:#} {RED}{}{RED:#}",
                    words::head(failure::message(also), self.line_max())
                )?;
            }
        }
        // Without a failure, the output is all there is to say: shown
        // from L2.
        let output = bad
            .result
            .map(|result| &result.output)
            .filter(|output| !output.is_empty())
            .filter(|_| level == Level::L3 || first.is_none());
        let (frames, locals) = match (level, first) {
            (Level::L3, Some(failure)) => (failure.frames.as_slice(), failure::locals(failure)),
            _ => (&[][..], &[][..]),
        };
        if !frames.is_empty() || output.is_some() || !locals.is_empty() {
            writeln!(self.out)?;
            self.frames(frames)?;
            if let Some(output) = output {
                self.output(output, OUTPUT_LINES)?;
            }
            self.locals(locals)?;
        }
        self.rows(bad, level)?;
        writeln!(self.out)?;
        self.group = Group::Block;
        Ok(())
    }

    /// `  failed in the test · 2 ms`, then for `aye show` the Python, the
    /// run and the file.
    fn second_line(&mut self, bad: &Bad<'_>) -> io::Result<()> {
        let mut parts = Vec::new();
        let word = words::outcome_words(bad.outcome);
        parts.push(match failure::phase_words(bad.failures.first()) {
            Some(phase) => format!("{word} in {phase}"),
            None => word.to_owned(),
        });
        if let Some(duration) = bad.duration {
            parts.push(words::spaced(duration));
        }
        if self.show.is_some() {
            if let Some(python) = self.facts.python(bad.worker) {
                parts.push(format!("python {python}"));
            }
            let run = self
                .context
                .run
                .or(self.facts.number)
                .map(|run| format!("run {run}"));
            match (run, &self.context.ago) {
                (Some(run), Some(ago)) => parts.push(format!("{run}, {ago}")),
                (Some(run), None) => parts.push(run),
                (None, Some(ago)) => parts.push(ago.clone()),
                (None, None) => {}
            }
            parts.push(
                match self.context.stale.get(bad.path) {
                    None => "file unchanged",
                    Some(FileChange::Edited) => "file changed since",
                    Some(FileChange::Gone) => "file gone",
                }
                .to_owned(),
            );
        }
        writeln!(self.out, "  {DIM}{}{DIM:#}", parts.join(" · "))
    }

    /// L3: every project frame, with its source, and each run of other
    /// frames as a line.
    fn frames(&mut self, frames: &[Frame]) -> io::Result<()> {
        let mut label = "frames";
        for item in failure::frame_items(frames) {
            match item {
                failure::FrameItem::Project(frame) => {
                    let mut line = self.location(frame.path.as_str(), frame.line);
                    if let Some(function) = &frame.function {
                        line = format!("{line} {DIM}in {function}{DIM:#}");
                    }
                    self.row(label, &line)?;
                    if let Some(source) = &frame.source {
                        write!(self.out, "{:pad$}", "", pad = LABEL + 6)?;
                        self.marked(source, frame.cols)?;
                    }
                }
                failure::FrameItem::Others { count, package } => {
                    let others = failure::others(count, &package);
                    self.row(label, &format!("{DIM}{others}{DIM:#}"))?;
                }
            }
            label = "";
        }
        Ok(())
    }

    /// L3: the innermost frame's local variables.
    fn locals(&mut self, locals: &[Local]) -> io::Result<()> {
        let mut label = "locals";
        for Local { name, value } in locals {
            self.row(label, &format!("{name} = {value}"))?;
            label = "";
        }
        Ok(())
    }

    /// The rows closing a block: hint, fixture, setup, history, not run,
    /// rerun, more.
    fn rows(&mut self, bad: &Bad<'_>, level: Level) -> io::Result<()> {
        let first = bad.failures.first();
        let mut rows: Vec<(&str, String)> = Vec::new();
        if let Some(hint) = first.and_then(|failure| failure.hint.as_deref()) {
            rows.push(("hint", hint.to_owned()));
        }
        if let Some(failure) = first
            && let Some(resource) = failure.resource.as_deref()
            && let (name, true) = failure::resource_name(resource, failure.phase)
        {
            rows.push(("fixture", name));
        }
        if let Some(result) = bad.result {
            if !result.resources.is_empty() {
                rows.push(("setup", failure::setup_chain(&result.resources)));
            }
            if let Some(history) = result.history.as_ref().and_then(words::history) {
                rows.push(("history", history));
            }
        }
        let any = !rows.is_empty()
            || !bad.not_run.is_empty()
            || bad.rerun.is_some()
            || level == Level::L2;
        if !any {
            return Ok(());
        }
        writeln!(self.out)?;
        for (label, value) in &rows {
            self.row(label, value)?;
        }
        let shown = if level == Level::L3 {
            bad.not_run.len()
        } else {
            NOT_RUN.min(bad.not_run.len())
        };
        let mut label = "not run";
        for (id, key) in &bad.not_run[..shown] {
            self.row(label, &format!("{KEY}{key}{KEY:#}  {id}"))?;
            label = "";
        }
        if bad.not_run.len() > shown {
            let more = bad.not_run.len() - shown;
            self.row("", &format!("{DIM}… {more} more{DIM:#}"))?;
        }
        if let Some(rerun) = &bad.rerun {
            self.row("rerun", rerun)?;
        }
        if level == Level::L2 {
            self.row(
                "more",
                &format!(
                    "aye show {KEY}{}{KEY:#} -vv   {DIM}(every frame, output, locals){DIM:#}",
                    bad.key
                ),
            )?;
        }
        Ok(())
    }

    /// `  label    value`.
    pub(super) fn row(&mut self, label: &str, value: &str) -> io::Result<()> {
        writeln!(self.out, "  {DIM}{label:<LABEL$}{DIM:#}  {value}")
    }

    /// The tail of each captured stream, `lines` lines each, a cut marked.
    pub(super) fn output(&mut self, output: &Captured, lines: usize) -> io::Result<()> {
        for (name, captured) in [
            ("stdout", &output.stdout),
            ("stderr", &output.stderr),
            ("log", &output.log),
        ] {
            if captured.trim().is_empty() {
                continue;
            }
            let tail = words::last_lines(captured, lines);
            let mut label = name;
            let earlier = tail.total - tail.lines.len();
            if earlier > 0 {
                let earlier = words::plural(earlier, "earlier line", "earlier lines");
                self.row(label, &format!("{DIM}… {earlier}{DIM:#}"))?;
                label = "";
            } else if output.truncated {
                self.row(label, &format!("{DIM}… (cut){DIM:#}"))?;
                label = "";
            }
            for line in tail.lines {
                self.row(label, &words::head(line, self.line_max()))?;
                label = "";
            }
        }
        Ok(())
    }
}
