//! What to call a resource: the one that failed ([`resource_name`]) and
//! the setup chain a test stood on ([`setup_chain`]).

use aye_events::{Phase, Resource};

/// What to call a resource that failed in `phase`, and whether it is a
/// fixture the project wrote: pytest's own for `setup_module` and its
/// kind (`_xunit_setup_module_fixture_<module>`) is called for what it
/// runs, `setup_module` or `teardown_module`.
pub(crate) fn resource_name(resource: &str, phase: Option<Phase>) -> (String, bool) {
    let Some(rest) = resource.strip_prefix("_xunit_setup_") else {
        return (resource.to_owned(), true);
    };
    let kind = rest.split_once("_fixture").map_or(rest, |(kind, _)| kind);
    let verb = if phase == Some(Phase::Teardown) {
        "teardown"
    } else {
        "setup"
    };
    (format!("{verb}_{kind}"), false)
}

/// The setup chain: `price_list (session) → cart (function)`.
pub(crate) fn setup_chain(resources: &[Resource]) -> String {
    resources
        .iter()
        .map(|resource| {
            let (name, _) = resource_name(&resource.name, None);
            format!("{name} ({})", resource.scope)
        })
        .collect::<Vec<_>>()
        .join(" → ")
}
