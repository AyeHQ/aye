//! How much text output shows of each thing.

/// Characters shown of a value, a source line or a line of output, below
/// L3.
pub(super) const LINE_MAX: usize = 300;
/// Lines of an explanation shown at L2.
pub(super) const DIFF_LINES: usize = 15;
/// Rows of a structured comparison shown at L2.
pub(super) const DIFFERENCES: usize = 10;
/// Lines of a message after its first, at L2.
pub(super) const MESSAGE_LINES: usize = 10;
/// Lines of each captured stream at L3.
pub(super) const OUTPUT_LINES: usize = 50;
/// Lines of a worker's error, below `-vv`.
pub(super) const WORKER_LINES: usize = 10;
/// Bad sub-outcomes of one test shown as blocks; the rest are lines.
pub(super) const SUB_BLOCKS: u32 = 3;
/// Tests a failed lifetime kept from running, listed under it at L2.
pub(super) const NOT_RUN: usize = 10;
/// Keys the details hint lists.
pub(super) const DETAILS: usize = 3;
/// Codes on the warnings line, reasons on the skipped line.
pub(super) const GROUPS: usize = 3;
/// Characters of a skip's reason on the skipped line.
pub(super) const REASON_MAX: usize = 100;
/// Characters of an operand's source naming a column.
pub(super) const SOURCE_LABEL: usize = 30;
/// The width of a row's label (`history`).
pub(super) const LABEL: usize = 7;
