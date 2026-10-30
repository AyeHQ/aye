//! Marks, decorators and `@pytest.mark.parametrize`, read from the source
//! (`_pytest/mark/structures.py`).
//! pytest's ids for parameter sets are in [`ids`].

mod mark;
mod parametrize;
mod pytestmark;

pub(crate) mod ids;

pub(crate) use mark::{Decorator, Mark, decorator, mark, timeout, usefixtures};
pub(crate) use parametrize::{EmptyMark, FixtureScope, SetTimeout, parametrize};
pub(crate) use pytestmark::{MarkList, mark_collection, pytestmark};
