//! The scanner: pytest's tokens, read lazily, so that a bad character
//! is reported only once parsing reaches it.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    LParen,
    RParen,
    Or,
    And,
    Not,
    Ident,
    Eof,
    Equal,
    Str,
    Comma,
    /// Where scanning failed; reaching it raises the error it carries.
    Error,
}

impl Kind {
    /// pytest's name for the token, as its messages print it.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::LParen => "left parenthesis",
            Self::RParen => "right parenthesis",
            Self::Or => "or",
            Self::And => "and",
            Self::Not => "not",
            Self::Ident => "identifier",
            Self::Eof => "end of input",
            Self::Equal => "=",
            Self::Str => "string literal",
            Self::Comma => ",",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct Token {
    pub(super) kind: Kind,
    /// The token's text; the message for `Kind::Error`.
    pub(super) value: String,
    pub(super) column: usize,
}

impl Token {
    fn new(kind: Kind, value: impl Into<String>, column: usize) -> Self {
        Self {
            kind,
            value: value.into(),
            column,
        }
    }
}

/// `\w` as Python's `re` reads it on `str`, plus pytest's extra characters.
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | ':' | '+' | '-' | '.' | '[' | ']' | '\\' | '/')
}

/// Every token up to the end, or up to and including an `Error` token.
pub(super) fn scan(input: &str) -> Vec<Token> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut pos = 0;
    while pos < chars.len() {
        let c = chars[pos];
        let column = pos + 1;
        let single = match c {
            '(' => Some(Kind::LParen),
            ')' => Some(Kind::RParen),
            '=' => Some(Kind::Equal),
            ',' => Some(Kind::Comma),
            _ => None,
        };
        if c == ' ' || c == '\t' {
            pos += 1;
        } else if let Some(kind) = single {
            tokens.push(Token::new(kind, c, column));
            pos += 1;
        } else if c == '\'' || c == '"' {
            let Some(len) = chars[pos + 1..].iter().position(|&q| q == c) else {
                let message = format!("closing quote \"{c}\" is missing");
                tokens.push(Token::new(Kind::Error, message, column));
                return tokens;
            };
            let literal = &chars[pos..pos + len + 2];
            if let Some(at) = literal.iter().position(|&b| b == '\\') {
                let message = "escaping with \"\\\" not supported in marker expression";
                tokens.push(Token::new(Kind::Error, message, column + at));
                return tokens;
            }
            tokens.push(Token::new(
                Kind::Str,
                literal.iter().collect::<String>(),
                column,
            ));
            pos += literal.len();
        } else if is_ident_char(c) {
            let len = chars[pos..]
                .iter()
                .take_while(|&&c| is_ident_char(c))
                .count();
            let value: String = chars[pos..pos + len].iter().collect();
            let kind = match value.as_str() {
                "or" => Kind::Or,
                "and" => Kind::And,
                "not" => Kind::Not,
                _ => Kind::Ident,
            };
            tokens.push(Token::new(kind, value, column));
            pos += len;
        } else {
            let message = format!("unexpected character \"{c}\"");
            tokens.push(Token::new(Kind::Error, message, column));
            return tokens;
        }
    }
    tokens.push(Token::new(Kind::Eof, "", chars.len() + 1));
    tokens
}
