//! `-m`: a tag expression, off only when empty, and what it matches of a
//! case: its tags, exactly.

use aye_events::Case;

use crate::expression::Expression;
use crate::selection::miss;
use crate::{SelectError, Truth};

pub(crate) fn tags_truth(expr: &Expression, case: &Case) -> Truth {
    let miss = miss(case);
    expr.evaluate(|ident, kwargs| {
        if !case.tags.iter().any(|tag| tag == ident) {
            miss
        } else if kwargs.is_empty() {
            Truth::True
        } else {
            Truth::Unknown
        }
    })
}

/// A tag expression, `None` when it turns selection off (empty).
pub(crate) fn parse_tags(expr: &str) -> Result<Option<Expression>, SelectError> {
    if expr.is_empty() {
        return Ok(None);
    }
    Expression::parse(expr)
        .map(Some)
        .map_err(|error| SelectError::Tags {
            expr: expr.to_string(),
            error,
        })
}
