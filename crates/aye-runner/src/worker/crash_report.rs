//! What of a worker's stderr goes into a crash report.

/// Lines and bytes of stderr put into a report.
const TAIL_LINES: usize = 30;
pub(super) const TAIL_BYTES: usize = 8000;
/// How faulthandler (and `Py_FatalError`) starts a dump.
const FATAL: &str = "Fatal Python error: ";

/// The part of a worker's stderr that goes into a report, at most
/// [`TAIL_LINES`] lines and [`TAIL_BYTES`] bytes.
///
/// A fatal error's dump (faulthandler's, on a crash) prints the innermost
/// frame first, so the last dump is kept from its start, the crashing
/// thread's stack first. Otherwise the end is kept, without the lines that
/// are only pytest's progress letters (its terminal output goes to stderr
/// too), which say nothing about a crash.
pub(super) fn report(text: &str) -> String {
    if let Some(at) = text.rfind(FATAL) {
        return head(&crashing_thread_first(&text[at..]));
    }
    let lines: Vec<&str> = text.lines().filter(|line| !is_progress(line)).collect();
    last(&lines.join("\n"))
}

/// pytest's progress letters, as a quiet terminal reporter prints them.
fn is_progress(line: &str) -> bool {
    let line = line.trim_end();
    !line.is_empty()
        && line
            .chars()
            .all(|c| matches!(c, '.' | 's' | 'x' | 'X' | 'F' | 'E'))
}

/// A fatal error's dump with the crashing thread's stack (`Current thread
/// …`) right after the header, before the other threads'.
fn crashing_thread_first(dump: &str) -> String {
    let lines: Vec<&str> = dump.lines().collect();
    let is_thread =
        |line: &&str| line.starts_with("Current thread ") || line.starts_with("Thread 0x");
    let Some(current) = lines.iter().position(|l| l.starts_with("Current thread ")) else {
        return dump.to_string();
    };
    let threads = lines.iter().position(is_thread).unwrap_or(current);
    let end = lines[current..]
        .iter()
        .position(|line| line.trim().is_empty())
        .map_or(lines.len(), |n| current + n);
    let mut ordered = lines[..threads].to_vec();
    ordered.extend_from_slice(&lines[current..end]);
    ordered.extend_from_slice(&lines[threads..current]);
    ordered.extend_from_slice(&lines[end..]);
    ordered.join("\n")
}

/// The start of `text`: at most [`TAIL_LINES`] lines and [`TAIL_BYTES`] bytes.
fn head(text: &str) -> String {
    let text = text.trim();
    let mut end = text.len().min(TAIL_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let lines: Vec<&str> = text[..end].lines().take(TAIL_LINES).collect();
    lines.join("\n")
}

/// The end of `text`: at most [`TAIL_LINES`] lines and [`TAIL_BYTES`] bytes.
fn last(text: &str) -> String {
    let text = text.trim_end();
    let mut start = text.len().saturating_sub(TAIL_BYTES);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let text = &text[start..];
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    #[test]
    fn a_crash_report_keeps_the_start_of_the_fatal_error_dump() {
        let mut text = String::from("early noise\n..F.Fatal Python error: Segmentation fault\n\n");
        text.push_str(
            "Thread 0x0a (most recent call first):\n  File \"helper.py\", line 3 in loop\n\n",
        );
        text.push_str("Current thread 0x0b (most recent call first):\n");
        text.push_str("  File \"ctypes/__init__.py\", line 541 in string_at\n");
        text.push_str("  File \"test_segv.py\", line 8 in test_segv\n");
        for i in 0..40 {
            writeln!(
                text,
                "  File \"pluggy/_callers.py\", line {i} in _multicall"
            )
            .unwrap();
        }
        text.push_str("\nExtension modules: _ctypes (total: 1)\n");
        let report = report(&text);
        let lines: Vec<&str> = report.lines().collect();
        assert_eq!(
            lines[0], "Fatal Python error: Segmentation fault",
            "{report}"
        );
        assert_eq!(lines[2], "Current thread 0x0b (most recent call first):");
        assert!(lines[3].contains("string_at") && lines[4].contains("test_segv.py\", line 8"));
        assert!(lines.len() <= TAIL_LINES, "{report}");
        assert!(!report.contains("early noise") && !report.contains("Thread 0x0a"));
    }

    #[test]
    fn a_crash_report_skips_pytest_progress_letters() {
        assert_eq!(report("."), "");
        assert_eq!(report("..F\nabout to crash\n.s"), "about to crash");
        // Text that merely contains those letters stays.
        assert_eq!(report("Fix.\n"), "Fix.");
    }

    #[test]
    fn a_dump_without_a_current_thread_is_kept_as_is() {
        let report = report("Fatal Python error: Aborted\n\nStack (most recent call first):\n  x");
        assert_eq!(
            report,
            "Fatal Python error: Aborted\n\nStack (most recent call first):\n  x"
        );
    }
}
