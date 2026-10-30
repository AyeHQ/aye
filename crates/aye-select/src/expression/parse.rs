//! The parser: recursive descent over the scanner's tokens, with pytest's
//! messages and columns.

use super::scan::{Kind, Token, scan};
use super::{Kwarg, MAX_DEPTH, Node, PYTHON_KEYWORDS, ParseError, Value, is_python_identifier};

pub(super) struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
}

impl Parser {
    pub(super) fn new(input: &str) -> Result<Self, ParseError> {
        let parser = Self {
            tokens: scan(input),
            pos: 0,
            depth: 0,
        };
        parser.raise()?;
        Ok(parser)
    }

    fn current(&self) -> &Token {
        &self.tokens[self.pos]
    }

    /// The scanning error, once parsing has reached it.
    fn raise(&self) -> Result<(), ParseError> {
        let token = self.current();
        if token.kind == Kind::Error {
            return Err(ParseError::new(token.column, token.value.clone()));
        }
        Ok(())
    }

    fn accept(&mut self, kind: Kind) -> Result<Option<Token>, ParseError> {
        if self.current().kind != kind {
            return Ok(None);
        }
        let token = self.current().clone();
        if kind != Kind::Eof {
            self.pos += 1;
            self.raise()?;
        }
        Ok(Some(token))
    }

    fn expect(&mut self, kind: Kind) -> Result<Token, ParseError> {
        match self.accept(kind)? {
            Some(token) => Ok(token),
            None => Err(self.reject(&[kind])),
        }
    }

    fn reject(&self, expected: &[Kind]) -> ParseError {
        let expected: Vec<&str> = expected.iter().map(|kind| kind.name()).collect();
        let current = self.current();
        ParseError::new(
            current.column,
            format!(
                "expected {}; got {}",
                expected.join(" OR "),
                current.kind.name()
            ),
        )
    }

    pub(super) fn expression(&mut self) -> Result<Node, ParseError> {
        if self.accept(Kind::Eof)?.is_some() {
            return Ok(Node::Empty);
        }
        let node = self.expr()?;
        self.expect(Kind::Eof)?;
        Ok(node)
    }

    fn expr(&mut self) -> Result<Node, ParseError> {
        let mut nodes = vec![self.and_expr()?];
        while self.accept(Kind::Or)?.is_some() {
            nodes.push(self.and_expr()?);
        }
        Ok(join(nodes, Node::Or))
    }

    fn and_expr(&mut self) -> Result<Node, ParseError> {
        let mut nodes = vec![self.not_expr()?];
        while self.accept(Kind::And)?.is_some() {
            nodes.push(self.not_expr()?);
        }
        Ok(join(nodes, Node::And))
    }

    fn not_expr(&mut self) -> Result<Node, ParseError> {
        if self.depth == MAX_DEPTH {
            return Err(ParseError::new(
                self.current().column,
                "expression nested too deeply",
            ));
        }
        self.depth += 1;
        let node = self.nested();
        self.depth -= 1;
        node
    }

    fn nested(&mut self) -> Result<Node, ParseError> {
        if self.accept(Kind::Not)?.is_some() {
            return Ok(Node::Not(Box::new(self.not_expr()?)));
        }
        if self.accept(Kind::LParen)?.is_some() {
            let node = self.expr()?;
            self.expect(Kind::RParen)?;
            return Ok(node);
        }
        if let Some(ident) = self.accept(Kind::Ident)? {
            let mut kwargs = Vec::new();
            if self.accept(Kind::LParen)?.is_some() {
                kwargs.push(self.kwarg()?);
                while self.accept(Kind::Comma)?.is_some() {
                    kwargs.push(self.kwarg()?);
                }
                self.expect(Kind::RParen)?;
            }
            return Ok(Node::Ident {
                name: ident.value,
                kwargs,
                column: ident.column,
            });
        }
        Err(self.reject(&[Kind::Not, Kind::LParen, Kind::Ident]))
    }

    fn kwarg(&mut self) -> Result<Kwarg, ParseError> {
        let name = self.expect(Kind::Ident)?;
        if !is_python_identifier(&name.value) {
            return Err(ParseError::new(
                name.column,
                format!("not a valid python identifier {}", name.value),
            ));
        }
        if PYTHON_KEYWORDS.contains(&name.value.as_str()) {
            return Err(ParseError::new(
                name.column,
                format!("unexpected reserved python keyword `{}`", name.value),
            ));
        }
        self.expect(Kind::Equal)?;
        let value = if let Some(literal) = self.accept(Kind::Str)? {
            let text = &literal.value;
            Value::Str(text[1..text.len() - 1].to_string())
        } else {
            let token = self.expect(Kind::Ident)?;
            value(&token)?
        };
        Ok(Kwarg {
            name: name.value,
            value,
        })
    }
}

fn join(mut nodes: Vec<Node>, make: fn(Vec<Node>) -> Node) -> Node {
    if nodes.len() == 1 {
        nodes.remove(0)
    } else {
        make(nodes)
    }
}

/// An unquoted keyword argument value: an integer, `True`, `False` or `None`.
fn value(token: &Token) -> Result<Value, ParseError> {
    let text = token.value.as_str();
    let digits = text.strip_prefix('-').unwrap_or(text);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        return text.parse().map(Value::Int).map_err(|_| {
            ParseError::new(token.column, format!("integer out of range \"{text}\""))
        });
    }
    match text {
        "True" => Ok(Value::Bool(true)),
        "False" => Ok(Value::Bool(false)),
        "None" => Ok(Value::None),
        _ => Err(ParseError::new(
            token.column,
            format!("unexpected character/s \"{text}\""),
        )),
    }
}
