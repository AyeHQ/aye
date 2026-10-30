//! The protocol stream: one JSON line per message each way, requests on
//! the worker's stdin and its replies and records on its stdout.

use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::mpsc::{RecvTimeoutError, Sender};
use std::time::Duration;

use aye_protocol::{Request, WorkerLine};

use super::Worker;
use super::timeouts::{DRAIN, EXIT_POLL};

/// One thing read from a worker's protocol stream.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Line(WorkerLine),
    /// A line that is not a protocol message.
    Garbage {
        line: String,
        error: String,
    },
    /// The stream ended: the worker exited or closed it.
    Eof,
}

impl fmt::Display for Incoming {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Line(line) => match serde_json::to_string(line) {
                Ok(text) => f.write_str(&text),
                Err(_) => write!(f, "{line:?}"),
            },
            Self::Garbage { line, .. } => f.write_str(line),
            Self::Eof => f.write_str("end of stream"),
        }
    }
}

impl Worker {
    pub fn send(&mut self, request: &Request) -> io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "stdin is closed"))?;
        let mut line = serde_json::to_vec(request)?;
        line.push(b'\n');
        stdin.write_all(&line)?;
        stdin.flush()
    }

    /// The next message, blocking. [`Incoming::Eof`] once the stream ended
    /// or the worker exited, and on every call after.
    pub fn recv(&self) -> Incoming {
        loop {
            if let Some(incoming) = self.recv_timeout(EXIT_POLL) {
                return incoming;
            }
        }
    }

    /// The next message, or `None` if nothing arrives within `timeout`.
    ///
    /// Once the worker has exited, the rest of what it wrote still arrives,
    /// then [`Incoming::Eof`], even while a process its tests started holds
    /// the stream open.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Incoming> {
        match self.lines.recv_timeout(timeout) {
            Ok(incoming) => Some(incoming),
            Err(RecvTimeoutError::Disconnected) => Some(Incoming::Eof),
            Err(RecvTimeoutError::Timeout) => self
                .exited()
                .then(|| self.lines.recv_timeout(DRAIN).unwrap_or(Incoming::Eof)),
        }
    }
}

/// Parse a worker's stdout into messages until it ends.
pub(super) fn read_lines(stdout: impl Read, tx: &Sender<Incoming>) {
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buf);
                let text = text.trim_end_matches(['\n', '\r']);
                if text.trim().is_empty() {
                    continue;
                }
                if tx.send(parse_line(text)).is_err() {
                    return;
                }
            }
        }
    }
    let _ = tx.send(Incoming::Eof);
}

/// One protocol line, or garbage with the reason.
pub fn parse_line(text: &str) -> Incoming {
    match serde_json::from_str::<WorkerLine>(text) {
        Ok(line) => Incoming::Line(line),
        Err(error) => Incoming::Garbage {
            line: text.to_string(),
            error: error.to_string(),
        },
    }
}

/// Whether a line that isn't a valid message was meant as a reply.
pub(crate) fn is_reply(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line).is_ok_and(|value| value.get("reply").is_some())
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use aye_events::{BatchIndex, Event};
    use aye_protocol::Reply;

    use super::*;

    #[test]
    fn lines_are_messages_or_garbage() {
        let (tx, rx) = mpsc::channel();
        let input = concat!(
            "{\"reply\":\"batch_done\",\"batch\":2}\n",
            "\n",
            "not json\r\n",
            "{\"batch\":2,\"event\":\"test_started\",\"id\":\"t.py::test_a\"}\n",
            "{\"no\":\"event\"}",
        );
        read_lines(input.as_bytes(), &tx);
        let got: Vec<Incoming> = rx.try_iter().collect();
        assert_eq!(got.len(), 5, "{got:?}");
        assert_eq!(
            got[0],
            Incoming::Line(WorkerLine::Reply(Reply::BatchDone {
                batch: BatchIndex::from_u32(2),
                status: None
            }))
        );
        assert!(matches!(&got[1], Incoming::Garbage { line, .. } if line == "not json"));
        let Incoming::Line(WorkerLine::Record(record)) = &got[2] else {
            panic!("expected a record: {:?}", got[2]);
        };
        assert_eq!(record.envelope.batch, Some(BatchIndex::from_u32(2)));
        assert!(matches!(record.event, Event::TestStarted { .. }));
        assert!(matches!(&got[3], Incoming::Garbage { .. }));
        assert_eq!(got[4], Incoming::Eof);
    }

    #[test]
    fn replies_are_told_from_other_lines() {
        assert!(is_reply(r#"{"reply":"batch_done","batch":"x"}"#));
        assert!(!is_reply(r#"{"event":"test_finished"}"#));
        assert!(!is_reply("reply"));
    }
}
