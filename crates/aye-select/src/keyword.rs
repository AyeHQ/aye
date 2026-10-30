//! `-k`: a keyword expression, off when blank, and what it matches of a
//! case: its keywords, or its path's parts, names and tags.

use crate::expression::Expression;
use crate::selection::miss;
use crate::selector::Parts;
use crate::{ParseError, SelectError, Truth};

pub(crate) fn keyword_truth(expr: &Expression, case: &Parts<'_>) -> Truth {
    let names = keyword_names(case);
    let miss = miss(case.case);
    expr.evaluate(|ident, _| {
        let ident = ident.to_lowercase();
        if names.iter().any(|name| name.contains(&ident)) {
            Truth::True
        } else {
            miss
        }
    })
}

/// The names a keyword expression matches, lowercase.
fn keyword_names(case: &Parts<'_>) -> Vec<String> {
    if !case.case.keywords.is_empty() {
        return case
            .case
            .keywords
            .iter()
            .map(|k| k.to_lowercase())
            .collect();
    }
    let mut names: Vec<String> = case.components.iter().map(|c| c.to_lowercase()).collect();
    if let Some((last, outer)) = case.names.split_last() {
        names.extend(outer.iter().map(|name| name.to_lowercase()));
        names.push(match case.params {
            Some(params) => format!("{last}[{params}]").to_lowercase(),
            None => last.to_lowercase(),
        });
    }
    names.extend(case.case.tags.iter().map(|tag| tag.to_lowercase()));
    names
}

/// A keyword expression, `None` when it turns selection off (empty or
/// blank).
pub(crate) fn parse_keyword(expr: &str) -> Result<Option<Expression>, SelectError> {
    let expr = expr.trim_start();
    let error = |error| SelectError::Keyword {
        expr: expr.to_string(),
        error,
    };
    if expr.is_empty() {
        return Ok(None);
    }
    let parsed = Expression::parse(expr).map_err(error)?;
    if let Some(column) = parsed.first_call() {
        return Err(error(ParseError {
            column,
            message: "keyword expressions do not support call parameters".to_string(),
        }));
    }
    Ok(Some(parsed))
}
