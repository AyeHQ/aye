//! The names of the two adapters pytest's rules concern, which each tests
//! the run for: with the doctest adapter in it, the files pytest's doctest
//! plugin would collect are the doctest adapter's, not the pytest
//! adapter's; with the pytest adapter in it, the doctest adapter applies
//! pytest's default text glob without a pytest configuration file. Each
//! adapter's own name is one of them.

use aye_events::AdapterName;

/// The pytest adapter's name.
pub const PYTEST: AdapterName = AdapterName::new_static("pytest");

/// The doctest adapter's name.
pub const DOCTEST: AdapterName = AdapterName::new_static("doctest");
