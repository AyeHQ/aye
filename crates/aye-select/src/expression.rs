//! pytest's `-k`/`-m` expression language, evaluated with Kleene's
//! three-valued logic:
//!
//! ```text
//! expression: expr? EOF
//! expr:       and_expr ('or' and_expr)*
//! and_expr:   not_expr ('and' not_expr)*
//! not_expr:   'not' not_expr | '(' expr ')' | ident kwargs?
//! ident:      (\w|:|\+|-|\.|\[|\]|\\|/)+
//! kwargs:     '(' name '=' value (',' name '=' value)* ')'
//! name:       a Python identifier that is not a keyword
//! value:      a quoted string without '\' | -?[0-9]+ | True | False | None
//! ```
//!
//! Errors carry pytest's messages and 1-based columns, and are found in the
//! order pytest finds them: its scanner is lazy, so a bad character after
//! the point where parsing fails is never reported.

use std::fmt;

use crate::Truth;
use parse::Parser;

mod parse;
mod scan;

/// The value of a keyword argument: `tag(name="x", n=1, flag=True)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Int(i64),
    Bool(bool),
    None,
}

/// One `name=value` in an identifier's call: `device(serial="123")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kwarg {
    pub name: String,
    pub value: Value,
}

/// A malformed expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// 1-based, in characters, as pytest reports it.
    pub column: usize,
    pub message: String,
}

impl ParseError {
    fn new(column: usize, message: impl Into<String>) -> Self {
        Self {
            column,
            message: message.into(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at column {}: {}", self.column, self.message)
    }
}

impl std::error::Error for ParseError {}

/// A parsed expression, with the text it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expression {
    input: String,
    node: Node,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    /// No tokens at all: pytest evaluates it to false.
    Empty,
    Ident {
        name: String,
        /// Empty without a call; a call always has at least one.
        kwargs: Vec<Kwarg>,
        column: usize,
    },
    Not(Box<Node>),
    And(Vec<Node>),
    Or(Vec<Node>),
}

/// Parentheses and `not` nested deeper than this are refused rather than
/// risk the stack (pytest manages about 300 parentheses).
const MAX_DEPTH: usize = 100;

impl Expression {
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        let node = Parser::new(input)?.expression()?;
        Ok(Self {
            input: input.to_string(),
            node,
        })
    }

    /// The text the expression was parsed from.
    pub fn as_str(&self) -> &str {
        &self.input
    }

    /// Evaluate with `matcher(ident, kwargs)`, which decides one identifier;
    /// `kwargs` is empty unless the identifier was called.
    pub fn evaluate(&self, mut matcher: impl FnMut(&str, &[Kwarg]) -> Truth) -> Truth {
        eval(&self.node, &mut matcher)
    }

    /// The column of the first identifier called with keyword arguments.
    pub(crate) fn first_call(&self) -> Option<usize> {
        fn find(node: &Node) -> Option<usize> {
            match node {
                Node::Empty => None,
                Node::Ident { kwargs, column, .. } => (!kwargs.is_empty()).then_some(*column),
                Node::Not(inner) => find(inner),
                Node::And(nodes) | Node::Or(nodes) => nodes.iter().find_map(find),
            }
        }
        find(&self.node)
    }
}

impl fmt::Display for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.input)
    }
}

fn eval(node: &Node, matcher: &mut dyn FnMut(&str, &[Kwarg]) -> Truth) -> Truth {
    match node {
        Node::Empty => Truth::False,
        Node::Ident { name, kwargs, .. } => matcher(name, kwargs),
        Node::Not(inner) => !eval(inner, matcher),
        Node::And(nodes) => {
            let mut truth = Truth::True;
            for node in nodes {
                truth = truth & eval(node, matcher);
                if truth == Truth::False {
                    break;
                }
            }
            truth
        }
        Node::Or(nodes) => {
            let mut truth = Truth::False;
            for node in nodes {
                truth = truth | eval(node, matcher);
                if truth == Truth::True {
                    break;
                }
            }
            truth
        }
    }
}

/// `str.isidentifier()`, approximated with Unicode's alphabetic and
/// alphanumeric classes.
pub(crate) fn is_python_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
        && chars.all(|c| c == '_' || c.is_alphanumeric())
}

/// `keyword.kwlist`.
const PYTHON_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Evaluate with `t*` true, `f*` false and anything else unknown.
    fn kleene(input: &str) -> Truth {
        Expression::parse(input)
            .unwrap()
            .evaluate(|name, _| match name.chars().next() {
                Some('t') => Truth::True,
                Some('f') => Truth::False,
                _ => Truth::Unknown,
            })
    }

    /// Evaluate with exactly `present` true.
    fn eval(input: &str, present: &[&str]) -> bool {
        Expression::parse(input)
            .unwrap()
            .evaluate(|name, _| present.contains(&name).into())
            == Truth::True
    }

    fn error(input: &str) -> (usize, String) {
        let err = Expression::parse(input).unwrap_err();
        (err.column, err.message)
    }

    fn kwargs(input: &str) -> Vec<Kwarg> {
        let mut seen = Vec::new();
        Expression::parse(input).unwrap().evaluate(|_, kwargs| {
            seen.extend_from_slice(kwargs);
            Truth::True
        });
        seen
    }

    #[test]
    fn precedence_and_parentheses() {
        assert!(eval("a or b and c", &["a"]));
        assert!(!eval("(a or b) and c", &["a"]));
        assert!(eval("not a and b", &["b"]));
        assert!(!eval("not (a and b)", &["a", "b"]));
        assert!(eval("not not a", &["a"]));
        assert!(eval("a and b and c or d", &["d"]));
        assert!(!eval("a and (b or c) and d", &["a", "b"]));
        assert!(eval("((a))", &["a"]));
    }

    #[test]
    fn identifiers_take_pytest_characters() {
        let expr = "test_x[1-2] or tests/a.py::b or a+b or a\\b or 1.5 or é";
        for ident in ["test_x[1-2]", "tests/a.py::b", "a+b", "a\\b", "1.5", "é"] {
            assert!(eval(expr, &[ident]), "{ident}");
        }
        assert!(
            eval("\tand_x\tor\tnotx ", &["notx"]),
            "keywords need a boundary"
        );
        assert!(eval("True or None", &["None"]));
    }

    #[test]
    fn empty_expression_is_false() {
        assert_eq!(kleene(""), Truth::False);
        assert_eq!(kleene("  \t"), Truth::False);
    }

    #[test]
    fn kleene_logic() {
        assert_eq!(kleene("u and f"), Truth::False);
        assert_eq!(kleene("u and t"), Truth::Unknown);
        assert_eq!(kleene("u or t"), Truth::True);
        assert_eq!(kleene("u or f"), Truth::Unknown);
        assert_eq!(kleene("not u"), Truth::Unknown);
        assert_eq!(kleene("not (u and f)"), Truth::True);
        assert_eq!(kleene("t and not (u or f)"), Truth::Unknown);
    }

    #[test]
    fn evaluation_stops_once_decided() {
        let expr = Expression::parse("f and x or t or y").unwrap();
        let mut asked = Vec::new();
        let truth = expr.evaluate(|name, _| {
            asked.push(name.to_string());
            Truth::from(name == "t")
        });
        assert_eq!(truth, Truth::True);
        assert_eq!(asked, ["f", "t"]);
    }

    #[test]
    fn calls_carry_their_keyword_arguments() {
        assert_eq!(
            kwargs(r#"device(serial="123", n=-4, big=007, ok=True, no=False, nil=None, s='')"#),
            [
                ("serial", Value::Str("123".into())),
                ("n", Value::Int(-4)),
                ("big", Value::Int(7)),
                ("ok", Value::Bool(true)),
                ("no", Value::Bool(false)),
                ("nil", Value::None),
                ("s", Value::Str(String::new())),
            ]
            .map(|(name, value)| Kwarg {
                name: name.into(),
                value,
            })
        );
        assert_eq!(kwargs("slow and slow(é=1)").len(), 1);
        assert_eq!(
            kwargs("slow(match=1)")[0].name,
            "match",
            "soft keywords pass"
        );
        let quoted = kwargs(r#"t(a="x y, (z) and 'w'")"#);
        assert_eq!(quoted[0].value, Value::Str("x y, (z) and 'w'".into()));
    }

    #[test]
    fn first_call_is_found_anywhere() {
        let column = |input: &str| Expression::parse(input).unwrap().first_call();
        assert_eq!(column("a or b"), None);
        assert_eq!(column("a or not (b and c(x=1))"), Some(17));
        assert_eq!(column("é(x=1)"), Some(1));
    }

    #[test]
    fn keeps_its_text() {
        let expr = Expression::parse(" slow and not db ").unwrap();
        assert_eq!(expr.as_str(), " slow and not db ");
        assert_eq!(expr.to_string(), " slow and not db ");
    }

    /// Every message and column here was checked against pytest 9.1.
    #[test]
    fn errors_match_pytest() {
        let not_or_paren_or_ident = "expected not OR left parenthesis OR identifier";
        for (input, column, message) in [
            (
                "a and",
                6,
                format!("{not_or_paren_or_ident}; got end of input"),
            ),
            (
                "not",
                4,
                format!("{not_or_paren_or_ident}; got end of input"),
            ),
            (
                "()",
                2,
                format!("{not_or_paren_or_ident}; got right parenthesis"),
            ),
            (
                "\"a\"",
                1,
                format!("{not_or_paren_or_ident}; got string literal"),
            ),
            (
                "a or (b",
                8,
                "expected right parenthesis; got end of input".into(),
            ),
            (
                "a)",
                2,
                "expected end of input; got right parenthesis".into(),
            ),
            ("a ,", 3, "expected end of input; got ,".into()),
            ("a = b", 3, "expected end of input; got =".into()),
            (
                "a 'b c' d",
                3,
                "expected end of input; got string literal".into(),
            ),
            ("a b $", 3, "expected end of input; got identifier".into()),
            ("$", 1, "unexpected character \"$\"".into()),
            ("a\nb", 2, "unexpected character \"\n\"".into()),
            ("é and $", 7, "unexpected character \"$\"".into()),
            ("a \"b", 3, "closing quote \"\"\" is missing".into()),
            ("slow(x='a", 8, "closing quote \"'\" is missing".into()),
            (
                "slow(x=\"a\\b\")",
                10,
                "escaping with \"\\\" not supported in marker expression".into(),
            ),
            ("slow(", 6, "expected identifier; got end of input".into()),
            (
                "slow()",
                6,
                "expected identifier; got right parenthesis".into(),
            ),
            (
                "slow(x=1,)",
                10,
                "expected identifier; got right parenthesis".into(),
            ),
            (
                "slow(x=1",
                9,
                "expected right parenthesis; got end of input".into(),
            ),
            (
                "x(a=1 b=2)",
                7,
                "expected right parenthesis; got identifier".into(),
            ),
            (
                "a(b=1)(c=2)",
                7,
                "expected end of input; got left parenthesis".into(),
            ),
            ("slow(x 1)", 8, "expected =; got identifier".into()),
            ("slow(1=2)", 6, "not a valid python identifier 1".into()),
            (
                "slow(if=2)",
                6,
                "unexpected reserved python keyword `if`".into(),
            ),
            (
                "slow(None=1)",
                6,
                "unexpected reserved python keyword `None`".into(),
            ),
            ("slow(x=y)", 8, "unexpected character/s \"y\"".into()),
            ("slow(x=-)", 8, "unexpected character/s \"-\"".into()),
            ("slow(x=+5)", 8, "unexpected character/s \"+5\"".into()),
            ("slow(x=1.5)", 8, "unexpected character/s \"1.5\"".into()),
        ] {
            assert_eq!(error(input), (column, message), "{input:?}");
        }
    }

    #[test]
    fn errors_display_like_pytest() {
        let err = Expression::parse("a and").unwrap_err();
        assert_eq!(
            err.to_string(),
            "at column 6: expected not OR left parenthesis OR identifier; got end of input"
        );
    }

    #[test]
    fn integers_beyond_64_bits_are_refused() {
        assert_eq!(
            error("slow(x=99999999999999999999)"),
            (8, "integer out of range \"99999999999999999999\"".into())
        );
    }

    #[test]
    fn deep_nesting_is_an_error_not_a_crash() {
        let deep = format!("{}a{}", "(".repeat(5000), ")".repeat(5000));
        assert_eq!(error(&deep), (101, "expression nested too deeply".into()));
        let nots = format!("{}a", "not ".repeat(5000));
        assert_eq!(error(&nots), (401, "expression nested too deeply".into()));
        let shallow = format!("{}a{}", "(".repeat(99), ")".repeat(99));
        assert!(eval(&shallow, &["a"]));
        assert!(eval(&format!("{}a", "not ".repeat(98)), &["a"]));
    }

    #[test]
    fn long_chains_do_not_nest() {
        let chain = vec!["a"; 100_000].join(" or ");
        assert_eq!(kleene(&chain), Truth::Unknown);
    }
}
