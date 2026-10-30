//! Marks read from a value: `pytestmark = …` and a `marks=` argument.

use aye_adapter::ast::Expr;
use aye_python_semantic::eval::deref;
use aye_python_semantic::namespace::{Binding, Bound};
use aye_python_semantic::resolve::{Lookup, Resolved};

use super::mark::{Mark, mark};

/// Marks read from a `pytestmark` value or a `marks=` argument.
#[derive(Debug, Default, Clone)]
pub(crate) struct MarkList<'s, 'a> {
    pub marks: Vec<Mark<'s, 'a>>,
    /// Some entry could not be read.
    pub unknown: bool,
}

/// Whether an expression that is not a mark may still evaluate to one
/// (a name imported from elsewhere, a call).
pub(super) fn maybe_mark<'a>(lookup: Lookup<'_, 'a>, expr: &'a Expr) -> bool {
    match expr {
        Expr::Call(_) => true,
        Expr::Name(_) | Expr::Attribute(_) => !matches!(
            lookup.resolve(expr),
            Resolved::Local(Bound {
                binding: Binding::Function(_) | Binding::Class(_),
                ..
            })
        ),
        _ => false,
    }
}

/// `pytestmark = …`: a mark or a list of marks. A tuple, or anything that
/// is not a mark, is a collection error in pytest.
pub(crate) fn pytestmark<'s, 'a>(
    lookup: Lookup<'s, 'a>,
    expr: &'a Expr,
) -> Result<MarkList<'s, 'a>, String> {
    let (expr, lookup) = deref(lookup, expr);
    match expr {
        Expr::List(list) => {
            let mut out = MarkList::default();
            for elt in &list.elts {
                if elt.is_starred_expr() {
                    out.unknown = true;
                    continue;
                }
                match mark(lookup, elt) {
                    Ok(Some(mark)) => out.marks.push(mark),
                    Ok(None) if maybe_mark(lookup, elt) => out.unknown = true,
                    Ok(None) => return Err("pytestmark holds something that is not a mark".into()),
                    Err(()) => out.unknown = true,
                }
            }
            Ok(out)
        }
        Expr::Tuple(_) => Err("pytestmark is a tuple; pytest wants a mark or a list".into()),
        other => match mark(lookup, other) {
            Ok(Some(mark)) => Ok(MarkList {
                marks: vec![mark],
                unknown: false,
            }),
            Ok(None) if !maybe_mark(lookup, other) => Err("pytestmark is not a mark".into()),
            Ok(None) | Err(()) => Ok(MarkList {
                marks: Vec::new(),
                unknown: true,
            }),
        },
    }
}

/// `marks=`: a mark, or a collection of marks. A set of marks raises
/// `TypeError` (marks aren't hashable), so it is never read.
pub(crate) fn mark_collection<'s, 'a>(lookup: Lookup<'s, 'a>, expr: &'a Expr) -> MarkList<'s, 'a> {
    let (expr, lookup) = deref(lookup, expr);
    let elts = match expr {
        Expr::List(list) => &list.elts[..],
        Expr::Tuple(tuple) => &tuple.elts[..],
        Expr::Set(_) => {
            return MarkList {
                marks: Vec::new(),
                unknown: true,
            };
        }
        other => {
            return match mark(lookup, other) {
                Ok(Some(mark)) => MarkList {
                    marks: vec![mark],
                    unknown: false,
                },
                _ => MarkList {
                    marks: Vec::new(),
                    unknown: true,
                },
            };
        }
    };
    let mut out = MarkList::default();
    for elt in elts {
        match mark(lookup, elt) {
            Ok(Some(mark)) => out.marks.push(mark),
            _ => out.unknown = true,
        }
    }
    out
}
