use std::io;

use aye_events::{Event, Record};

use super::streams;
use crate::{JsonLines, Level, Reporter, Tee};

/// Fails on every record, and counts how often it was asked.
struct Broken<'a>(&'a mut u32);

impl Reporter for Broken<'_> {
    fn report(&mut self, _: &Record) -> io::Result<()> {
        *self.0 += 1;
        Err(io::Error::other("broken pipe"))
    }

    fn finish(&mut self) -> io::Result<()> {
        Err(io::Error::other("still broken"))
    }
}

#[test]
fn every_reporter_sees_every_record() {
    let records = streams::mixed();
    let (mut all, mut verbose, mut calls) = (Vec::new(), Vec::new(), 0);
    {
        let mut tee = Tee::new()
            .with(Broken(&mut calls))
            .with(JsonLines::new(&mut all, Level::Engine));
        tee.push(JsonLines::new(&mut verbose, Level::Verbose));
        assert_eq!(tee.len(), 3);
        assert!(!tee.is_empty());
        for record in &records {
            let error = tee.report(record).unwrap_err();
            assert_eq!(error.to_string(), "broken pipe");
        }
        assert_eq!(tee.finish().unwrap_err().to_string(), "still broken");
    }
    assert_eq!(calls, u32::try_from(records.len()).unwrap());
    // A line per bad outcome, and the summary.
    assert_eq!(String::from_utf8(all).unwrap().lines().count(), 6 + 1);
    assert!(
        String::from_utf8(verbose)
            .unwrap()
            .ends_with("\"more\":\"aye show <key> --output json\"}\n")
    );
}

#[test]
fn an_empty_tee_does_nothing() {
    let mut tee = Tee::new();
    assert!(tee.is_empty());
    tee.report(&Record::new(Event::CollectionStarted)).unwrap();
    tee.finish().unwrap();
}

#[test]
fn boxed_and_borrowed_reporters_are_reporters() {
    let mut out = Vec::new();
    {
        let mut json = JsonLines::new(&mut out, Level::Engine);
        let mut boxed: Box<dyn Reporter> = Box::new(&mut json);
        boxed
            .report(&Record::new(Event::RunFinished {
                exit_code: aye_events::ExitCode::Ok,
                counts: aye_events::Counts::default(),
                duration: std::time::Duration::ZERO,
                stopped: None,
            }))
            .unwrap();
        boxed.finish().unwrap();
    }
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "{\"type\":\"summary\",\"exit\":0,\"s\":0.0}\n"
    );
}
