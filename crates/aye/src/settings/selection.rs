//! What static selection keeps ([`resolve`]): the positional arguments,
//! the `-k` and `-m` that count for each adapter's cases, and each
//! adapter's `--deselect`.

use aye_adapter::FrameworkDefaults;
use aye_events::AdapterName;
use aye_select::Selection;

use super::each;
use crate::commands::run::select::{Pick, pick, unread};
use crate::framework_args::passed;

/// What static selection keeps: the positional arguments' `selectors`, the
/// `-k` and `-m` expressions [`pick`] picks, and each adapter's
/// `--deselect` prefixes ([`FrameworkDefaults::deselect`]). A malformed
/// expression on the command line is a usage error, even where the
/// arguments after `--` override it; the framework's own, should aye fail
/// to parse it, drops no case.
pub(super) fn resolve<'s>(
    keyword: Option<&str>,
    tags: Option<&str>,
    framework_args: &[String],
    selectors: impl IntoIterator<Item = &'s str>,
    defaults: &[(AdapterName, FrameworkDefaults)],
) -> Result<Selection, aye_select::SelectError> {
    if let Some(expr) = keyword {
        Selection::default().with_keyword(expr)?;
    }
    if let Some(expr) = tags {
        Selection::default().with_tags(expr)?;
    }
    let mut selection = Selection::default().with_ids(selectors);
    let own_keyword = |d: &FrameworkDefaults| d.keyword.clone();
    match pick(keyword, passed(framework_args, 'k')) {
        Pick::Given(expr) => {
            selection = selection.clone().with_keyword(&expr).unwrap_or(selection);
        }
        Pick::Unknown => {
            for (name, expr) in unread(keyword, defaults, own_keyword) {
                selection = selection
                    .clone()
                    .with_own_keyword(name.clone(), &expr)
                    .unwrap_or(selection);
            }
        }
        Pick::Own => {
            for (name, expr) in each(defaults, own_keyword) {
                selection = selection
                    .clone()
                    .with_own_keyword(name.clone(), &expr)
                    .unwrap_or(selection);
            }
        }
    }
    let own_tags = |d: &FrameworkDefaults| d.tags.clone();
    match pick(tags, passed(framework_args, 'm')) {
        Pick::Given(expr) => {
            selection = selection.clone().with_tags(&expr).unwrap_or(selection);
        }
        Pick::Unknown => {
            for (name, expr) in unread(tags, defaults, own_tags) {
                selection = selection
                    .clone()
                    .with_own_tags(name.clone(), &expr)
                    .unwrap_or(selection);
            }
        }
        Pick::Own => {
            for (name, expr) in each(defaults, own_tags) {
                selection = selection
                    .clone()
                    .with_own_tags(name.clone(), &expr)
                    .unwrap_or(selection);
            }
        }
    }
    for (name, defaults) in defaults {
        selection = selection.with_deselect(name.clone(), defaults.deselect.iter().cloned());
    }
    Ok(selection)
}
