//! Python's `re`: which patterns `re.compile` accepts.

use crate::identifier::is_ascii_identifier;

/// Whether `re.compile` surely accepts a `str` pattern, without warnings,
/// on every Python aye supports: a conservative subset of the syntax
/// (literals, escapes, classes, plain, non-capturing, named and lookahead
/// groups, alternation, quantifiers). Anything else is left to Python.
pub fn compiles(pattern: &str) -> bool {
    accepts(&pattern.chars().collect::<Vec<_>>(), false)
}

/// [`compiles`] for a `bytes` pattern, whose escapes take no `\u` or `\U`.
pub fn compiles_bytes(pattern: &[u8]) -> bool {
    accepts(
        &pattern.iter().map(|b| char::from(*b)).collect::<Vec<_>>(),
        true,
    )
}

fn accepts(chars: &[char], bytes: bool) -> bool {
    let mut parser = Regex {
        chars,
        at: 0,
        bytes,
        names: Vec::new(),
        groups: 0,
    };
    parser.alternation(0).is_some() && parser.at == chars.len()
}

/// What the last piece of a branch was, for the quantifier after it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Piece {
    /// Nothing yet: nothing to repeat.
    Nothing,
    /// `^`, `$`, `\b`, a lookahead: not repeated here.
    Assertion,
    Atom,
    /// Already repeated: another quantifier is an error, or possessive.
    Repeated,
}

/// A recursive-descent reader of `re` patterns (`_parser.py`), refusing
/// whatever it is not sure about.
struct Regex<'p> {
    chars: &'p [char],
    at: usize,
    bytes: bool,
    names: Vec<String>,
    groups: usize,
}

impl Regex<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += 1;
        Some(c)
    }

    fn alternation(&mut self, depth: usize) -> Option<()> {
        if depth > 32 {
            return None;
        }
        loop {
            self.branch(depth)?;
            if self.peek() != Some('|') {
                return Some(());
            }
            self.at += 1;
        }
    }

    fn branch(&mut self, depth: usize) -> Option<()> {
        let mut last = Piece::Nothing;
        while let Some(c) = self.peek() {
            if last == Piece::Repeated && matches!(c, '*' | '+' | '?' | '{') {
                return None;
            }
            last = match c {
                '|' | ')' => return Some(()),
                '*' | '+' | '?' => {
                    self.at += 1;
                    self.repeated(last)?
                }
                '{' => {
                    if self.quantifier()? {
                        self.repeated(last)?
                    } else {
                        Piece::Atom
                    }
                }
                '(' => {
                    self.at += 1;
                    self.group(depth)?
                }
                '[' => {
                    self.at += 1;
                    self.class()?;
                    Piece::Atom
                }
                '\\' => {
                    self.at += 1;
                    self.escape()?
                }
                '^' | '$' => {
                    self.at += 1;
                    Piece::Assertion
                }
                _ => {
                    self.at += 1;
                    Piece::Atom
                }
            };
        }
        Some(())
    }

    /// After a quantifier: the piece it repeats must be an atom; a lazy `?`
    /// may follow.
    fn repeated(&mut self, last: Piece) -> Option<Piece> {
        if last != Piece::Atom {
            return None;
        }
        if self.peek() == Some('?') {
            self.at += 1;
        }
        Some(Piece::Repeated)
    }

    /// At `{`: whether it starts a quantifier (consumed), or is a literal
    /// brace (only it consumed); `None` for a bad one.
    fn quantifier(&mut self) -> Option<bool> {
        let start = self.at;
        self.at += 1;
        if self.peek() == Some('}') {
            return Some(false);
        }
        let lo = self.digits();
        let hi = if self.peek() == Some(',') {
            self.at += 1;
            self.digits()
        } else {
            lo.clone()
        };
        if self.peek() != Some('}') {
            self.at = start + 1;
            return Some(false);
        }
        self.at += 1;
        let bound = |digits: &str| -> Option<Option<u32>> {
            if digits.is_empty() {
                Some(None)
            } else {
                digits.parse::<u32>().ok().filter(|n| *n < 65_536).map(Some)
            }
        };
        let lo = bound(&lo)?.unwrap_or(0);
        match bound(&hi)? {
            Some(hi) if hi < lo => None,
            _ => Some(true),
        }
    }

    fn digits(&mut self) -> String {
        let mut out = String::new();
        while let Some(c) = self.peek().filter(char::is_ascii_digit) {
            out.push(c);
            self.at += 1;
        }
        out
    }

    /// After `(`.
    fn group(&mut self, depth: usize) -> Option<Piece> {
        let mut piece = Piece::Atom;
        if self.peek() == Some('?') {
            self.at += 1;
            match self.next()? {
                ':' => {}
                '=' | '!' => piece = Piece::Assertion,
                'P' => {
                    if self.next()? != '<' {
                        return None;
                    }
                    let mut name = String::new();
                    loop {
                        match self.next()? {
                            '>' => break,
                            c => name.push(c),
                        }
                    }
                    if !is_ascii_identifier(&name) || self.names.contains(&name) {
                        return None;
                    }
                    self.names.push(name);
                    self.groups += 1;
                }
                // Flags, lookbehinds, comments, backreferences, atomic
                // groups, conditionals.
                _ => return None,
            }
        } else {
            self.groups += 1;
        }
        if self.groups > 100 {
            return None;
        }
        self.alternation(depth + 1)?;
        (self.next()? == ')').then_some(piece)
    }

    /// After `\`, outside a class.
    fn escape(&mut self) -> Option<Piece> {
        match self.next()? {
            'd' | 'D' | 's' | 'S' | 'w' | 'W' => Some(Piece::Atom),
            'A' | 'Z' | 'b' | 'B' => Some(Piece::Assertion),
            c => self.literal_escape(c).map(|_| Piece::Atom),
        }
    }

    /// An escape standing for one character, after `\` and `c`: its code.
    fn literal_escape(&mut self, c: char) -> Option<u32> {
        match c {
            'a' => Some(7),
            'b' => Some(8),
            't' => Some(9),
            'n' => Some(10),
            'v' => Some(11),
            'f' => Some(12),
            'r' => Some(13),
            'x' => self.hex(2),
            'u' if !self.bytes => self.hex(4),
            'U' if !self.bytes => self.hex(8).filter(|c| *c <= 0x10_ffff),
            '0' => {
                let mut code = 0;
                for _ in 0..2 {
                    match self.peek().and_then(|d| d.to_digit(8)) {
                        Some(digit) => {
                            code = code * 8 + digit;
                            self.at += 1;
                        }
                        None => break,
                    }
                }
                Some(code)
            }
            // Other letters are errors, digits are group references.
            c if c.is_ascii_alphanumeric() => None,
            c => Some(u32::from(c)),
        }
    }

    fn hex(&mut self, count: usize) -> Option<u32> {
        let mut code = 0;
        for _ in 0..count {
            code = code * 16 + self.next()?.to_digit(16)?;
        }
        Some(code)
    }

    /// After `[`.
    fn class(&mut self) -> Option<()> {
        // `[[`: "possible nested set" (a `FutureWarning`).
        if self.peek() == Some('[') {
            return None;
        }
        if self.peek() == Some('^') {
            self.at += 1;
        }
        let mut first = true;
        loop {
            let c = self.next()?;
            if c == ']' && !first {
                return Some(());
            }
            first = false;
            let lo = self.class_item(c)?;
            if self.peek() == Some('-') {
                self.at += 1;
                let that = self.next()?;
                if that == ']' {
                    return Some(());
                }
                if that == '-' {
                    return None;
                }
                match (lo, self.class_item(that)?) {
                    (Member::Char(lo), Member::Char(hi)) if lo <= hi => {}
                    _ => return None,
                }
            }
        }
    }

    /// One member of a class, after `c`; `None` when Python may refuse or
    /// warn.
    fn class_item(&mut self, c: char) -> Option<Member> {
        match c {
            '\\' => match self.next()? {
                'd' | 'D' | 's' | 'S' | 'w' | 'W' => Some(Member::Category),
                e => self.literal_escape(e).map(Member::Char),
            },
            // Nested sets and set operations warn.
            '[' => None,
            '-' | '&' | '~' | '|' if self.peek() == Some(c) => None,
            c => Some(Member::Char(u32::from(c))),
        }
    }
}

/// A member of a regex class.
#[derive(Clone, Copy)]
enum Member {
    /// One character, by code.
    Char(u32),
    /// `\d` and the like.
    Category,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Patterns [`compiles`] accepts; each compiles, without warnings,
    /// on Python 3.10 to 3.14.
    const GOOD_PATTERNS: &[&str] = &[
        "",
        "abc",
        "a|b|",
        "^a.b$",
        r"\d+\s*\w?\D\S\W",
        r"\.\*\(\)\[\]\{\}\\\-\ ",
        r"a{2}b{1,}c{,3}d{2,5}?e*?f+?g??",
        "a{}b{x}c{1,2",
        r"[abc][^abc][a-z0-9_][\d\s][-a][a-][]a][^]a]",
        r"[\x00-\x7f\u00e9\n\t\\]",
        r"(a)(?:b)(?P<name>c)(?=d)(?!e)()",
        r"\x41\u00e9\U0001F600\0\012\a\f\v\b",
        "é+",
    ];

    /// Patterns it refuses: errors, warnings, or syntax it doesn't follow.
    const BAD_PATTERNS: &[&str] = &[
        "(",
        ")",
        "a)",
        "*a",
        "a**",
        "a*+",
        "a{2}{3}",
        "a???",
        "|*",
        "^*",
        r"\b+",
        "[",
        "[]",
        "[z-a]",
        r"[a-\d]",
        "[[a]",
        "[a--b]",
        "[a&&b]",
        "[a||b]",
        "[a~~b]",
        r"\p",
        r"\1",
        r"\z",
        r"\x4",
        r"\u12",
        r"\N{LATIN SMALL LETTER A}",
        "\\",
        "(?i)a",
        "(?<=a)b",
        "(?#c)",
        "(?P=n)",
        "(?P<1>a)",
        "(?P<n>a)(?P<n>b)",
        "(?>a)",
        "a{3,2}",
        "a{99999}",
        "(?=a)*",
    ];

    #[test]
    fn regex_patterns_compile_when_aye_says_so() {
        for pattern in GOOD_PATTERNS {
            assert!(compiles(pattern), "{pattern:?}");
        }
        for pattern in BAD_PATTERNS {
            assert!(!compiles(pattern), "{pattern:?}");
        }
        assert!(compiles_bytes(b"[\x00-\xff]+"));
        assert!(!compiles_bytes(br"\u00e9"));
    }

    #[test]
    fn regex_patterns_compile_in_python() {
        let Some(python) = aye_test::test_python(aye_test::Needs::Stdlib) else {
            return;
        };
        let script = r#"
import json, re, sys, warnings
warnings.simplefilter("error")
for pattern in json.loads(sys.stdin.read()):
    try:
        re.compile(pattern)
    except Exception as error:
        print(f"{pattern!r}: {error}")
"#;
        let mut child = std::process::Command::new(python)
            .args(["-c", script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let input = serde_json::to_string(GOOD_PATTERNS).unwrap();
        std::io::Write::write_all(child.stdin.as_mut().unwrap(), input.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    }
}
