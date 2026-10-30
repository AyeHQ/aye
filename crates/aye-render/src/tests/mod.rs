use aye_events::Record;

use aye_stream::Consumer;

mod streams;

mod adapters;
mod github;
mod json;
mod junit;
mod ladder;
mod show;
mod tee;
mod text;

/// Feeds a recorded stream through a reporter.
fn feed(reporter: &mut impl Consumer, records: &[Record]) {
    for record in records {
        reporter.consume(record).unwrap();
    }
    reporter.finish().unwrap();
}

/// Lines, each ended by a newline.
fn lines(lines: &[&str]) -> String {
    lines.iter().flat_map(|line| [*line, "\n"]).collect()
}
