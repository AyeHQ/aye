//! What consumes the log ([`Consumer`]), how one stream reaches several
//! ([`Tee`]), and what a consumer's error does to the others: a stdout
//! consumer stops at its first ([`StopAfterError`]), a file written at the
//! end says its own and fails the run ([`FailOnFinish`]).

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aye_events::Record;

/// A consumer of the run's log: a renderer, history, the run store.
pub trait Consumer {
    /// Fold one record.
    fn consume(&mut self, record: &Record) -> io::Result<()>;

    /// The stream has ended: write anything held back and flush.
    fn finish(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<C: Consumer + ?Sized> Consumer for Box<C> {
    fn consume(&mut self, record: &Record) -> io::Result<()> {
        (**self).consume(record)
    }

    fn finish(&mut self) -> io::Result<()> {
        (**self).finish()
    }
}

impl<C: Consumer + ?Sized> Consumer for &mut C {
    fn consume(&mut self, record: &Record) -> io::Result<()> {
        (**self).consume(record)
    }

    fn finish(&mut self) -> io::Result<()> {
        (**self).finish()
    }
}

/// Fans one stream out to several consumers. Every consumer sees every
/// record even when another fails; the first error is returned.
#[derive(Default)]
pub struct Tee<'a> {
    consumers: Vec<Box<dyn Consumer + Send + 'a>>,
}

impl<'a> Tee<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, consumer: impl Consumer + Send + 'a) {
        self.consumers.push(Box::new(consumer));
    }

    #[must_use]
    pub fn with(mut self, consumer: impl Consumer + Send + 'a) -> Self {
        self.push(consumer);
        self
    }

    pub fn len(&self) -> usize {
        self.consumers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.consumers.is_empty()
    }

    fn each(
        &mut self,
        mut call: impl FnMut(&mut (dyn Consumer + Send + 'a)) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut first = Ok(());
        for consumer in &mut self.consumers {
            if let Err(error) = call(consumer.as_mut())
                && first.is_ok()
            {
                first = Err(error);
            }
        }
        first
    }
}

impl Consumer for Tee<'_> {
    fn consume(&mut self, record: &Record) -> io::Result<()> {
        self.each(|consumer| consumer.consume(record))
    }

    fn finish(&mut self) -> io::Result<()> {
        self.each(Consumer::finish)
    }
}

/// A consumer writing to stdout. After its first error (a closed pipe) it
/// writes nothing more, so the error is reported once and every other
/// consumer still sees the whole stream.
pub struct StopAfterError<C> {
    inner: C,
    failed: bool,
}

impl<C> StopAfterError<C> {
    pub const fn new(inner: C) -> Self {
        Self {
            inner,
            failed: false,
        }
    }
}

impl<C: Consumer> Consumer for StopAfterError<C> {
    fn consume(&mut self, record: &Record) -> io::Result<()> {
        if self.failed {
            return Ok(());
        }
        self.inner
            .consume(record)
            .inspect_err(|_| self.failed = true)
    }

    fn finish(&mut self) -> io::Result<()> {
        if self.failed {
            return Ok(());
        }
        self.inner.finish().inspect_err(|_| self.failed = true)
    }
}

/// A consumer writing a file when the stream ends: its error is handed to
/// `say` (it names the file), and remembered in `failed`, for the exit
/// code; the stream's end goes on as if it hadn't failed.
pub struct FailOnFinish<C, S> {
    inner: C,
    failed: Arc<AtomicBool>,
    say: S,
}

impl<C, S: FnMut(io::Error)> FailOnFinish<C, S> {
    pub const fn new(inner: C, failed: Arc<AtomicBool>, say: S) -> Self {
        Self { inner, failed, say }
    }
}

impl<C: Consumer, S: FnMut(io::Error)> Consumer for FailOnFinish<C, S> {
    fn consume(&mut self, record: &Record) -> io::Result<()> {
        self.inner.consume(record)
    }

    fn finish(&mut self) -> io::Result<()> {
        if let Err(err) = self.inner.finish() {
            (self.say)(err);
            self.failed.store(true, Ordering::Relaxed);
        }
        Ok(())
    }
}
