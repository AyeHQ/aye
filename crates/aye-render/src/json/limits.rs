//! How much a JSON line holds of each thing, in characters unless stated.

pub(super) const ID_MAX: usize = 1000;
pub(super) const PATH_MAX: usize = 500;
pub(super) const CODE_MAX: usize = 300;
pub(super) const SUMMARY_MAX: usize = 200;
pub(super) const VALUE_MAX: usize = 200;
pub(super) const OPERAND_MAX: usize = 500;
pub(super) const SOURCE_MAX: usize = 200;
pub(super) const MESSAGE_MAX: usize = 500;
pub(super) const MESSAGE_LINES: usize = 10;
pub(super) const HINT_MAX: usize = 500;
pub(super) const FIXTURE_MAX: usize = 200;
pub(super) const SETUP: usize = 30;
pub(super) const DIFF_LINES: usize = 30;
pub(super) const DIFF_LINE_MAX: usize = 200;
/// Entries of a structured comparison at L2.
pub(super) const DIFFERENCES: usize = 20;
pub(super) const FRAME_MAX: usize = 300;
pub(super) const OUTPUT_LINES: usize = 50;
pub(super) const OUTPUT_MAX: usize = 4000;
pub(super) const LOCAL_MAX: usize = 200;
pub(super) const ALSO: usize = 3;
pub(super) const BLOCKED: usize = 20;
pub(super) const RERUN_MAX: usize = 1000;
pub(super) const COLLECTION_ERROR_MAX: usize = 1000;
pub(super) const DIAGNOSTIC_CODE_MAX: usize = 100;
pub(super) const WORKER_ERROR_MAX: usize = 2000;
pub(super) const REASON_MAX: usize = 500;
/// Keys in the summary's `failures`.
pub(super) const FAILURES: usize = 50;
/// Bad sub-outcomes of one test at their level; later ones are at L1.
pub(super) const SUB_LINES: usize = 3;
