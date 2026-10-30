//! What the binary's end-to-end tests and the comparison harness share
//! beside aye-test: the runs a project's store keeps, read through
//! aye-history, which aye-test may not depend on.

use std::collections::HashSet;
use std::path::Path;

use aye_history::RunStore;
use serde_json::Value;

/// The run store of the project rooted at `root`.
fn store(root: &Path) -> RunStore {
    RunStore::new(aye_test::runs_dir(root))
}

/// The runs the project rooted at `root` keeps.
pub(crate) fn runs_in(root: &Path) -> HashSet<String> {
    store(root)
        .runs()
        .into_iter()
        .map(|meta| meta.run.to_string())
        .collect()
}

/// The log of the run the project rooted at `root` kept since `before`,
/// every record as the run wrote it (`aye show --log`; what `--output json
/// -vvv` printed before the detail ladder); `None` when it kept none.
pub(crate) fn kept(root: &Path, before: &HashSet<String>) -> Option<Vec<Value>> {
    let store = store(root);
    let meta = store
        .runs()
        .into_iter()
        .find(|meta| !before.contains(&meta.run.to_string()))?;
    let log = store.log(&meta.run.to_string()).unwrap();
    Some(
        log.iter()
            .map(|record| serde_json::to_value(record).unwrap())
            .collect(),
    )
}
