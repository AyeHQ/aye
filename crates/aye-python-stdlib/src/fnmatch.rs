//! Python's `fnmatch`, and pytest's `fnmatch_ex` and name options, which
//! match with it.
//!
//! A pattern is compiled into tokens ([`FnPattern`]) and matched by one
//! iterative matcher, `*` backtracking to the last star. How its `[...]`
//! sets are read is a [`SetTranslation`]: CPython's, or the reading aye's
//! pytest and doctest adapters have always had, which differs from it for
//! a set whose first item is a reversed range followed by `!`. Each
//! adapter asks for the reading it had.
//!
//! `globset` is not used on purpose: it reads `{a,b}` as alternation, while
//! `fnmatch` treats braces literally (pytest's default `norecursedirs` holds
//! the literal `{arch}`).

use std::path::Path;

/// How a pattern's `[...]` sets are read. Finding a set's closing `]` (a
/// `]` right after the `[` or `[!` belongs to the set; with none, the `[`
/// is literal) is the same in both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetTranslation {
    /// As `fnmatch._translate` reads them (Python 3.10 to 3.13 agree):
    /// hyphens between chunks make ranges, empty ranges are dropped, an
    /// empty set matches nothing and `[!]` matches any character. The set
    /// is then read as `re` reads a character class. The unittest adapter
    /// reads sets so.
    CPython,
    /// A leading `!` negates; then each `x-y` is a range, a reversed one
    /// matching nothing, and any other character stands for itself. The
    /// pytest and doctest adapters read sets so. It differs from CPython's
    /// reading for a set whose first item is a reversed range followed by
    /// `!`: `[z-a!b]` is any character but `b` to CPython, `!` or `b` here.
    Legacy,
}

/// `fnmatch.fnmatch(name, pattern)`: `*`, `?`, `[seq]` and `[!seq]`, no
/// escapes, sets read the `sets` way. Both go through `os.path.normcase`
/// first, which on Windows folds case and turns `/` into `\`.
pub fn fnmatch(name: &str, pattern: &str, sets: SetTranslation) -> bool {
    if cfg!(windows) {
        return FnPattern::new(&nt_normcase(pattern), sets).matches(&nt_normcase(name));
    }
    FnPattern::new(pattern, sets).matches(name)
}

/// `ntpath.normcase`: lower case, `\` separators.
fn nt_normcase(s: &str) -> String {
    s.replace('/', "\\").to_lowercase()
}

/// pytest's `fnmatch_ex`: a pattern without a separator matches the file
/// name; one with a separator matches the whole path, anchored anywhere when
/// the path is absolute and the pattern is not. On Windows, a pattern
/// without `\` has its `/` read as `\`.
pub fn fnmatch_ex(pattern: &str, path: &Path, sets: SetTranslation) -> bool {
    let sep = std::path::MAIN_SEPARATOR;
    let pattern = if cfg!(windows) && !pattern.contains(sep) && pattern.contains('/') {
        pattern.replace('/', &sep.to_string())
    } else {
        pattern.to_string()
    };
    if pattern.contains(sep) {
        let full = path.to_string_lossy();
        if path.is_absolute() && !Path::new(&pattern).is_absolute() {
            fnmatch(&full, &format!("*{sep}{pattern}"), sets)
        } else {
            fnmatch(&full, &pattern, sets)
        }
    } else {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        fnmatch(&name, &pattern, sets)
    }
}

/// Whether a pytest `python_classes` or `python_functions` option matches
/// `name`: a prefix, or an `fnmatch` pattern when it holds glob characters.
pub fn matches_prefix_or_glob(options: &[String], name: &str, sets: SetTranslation) -> bool {
    options.iter().any(|option| {
        name.starts_with(option.as_str())
            || (option.contains(['*', '?', '[']) && fnmatch(name, option, sets))
    })
}

/// A pattern compiled the way `fnmatch.translate` compiles it, with its
/// sets read one [`SetTranslation`]'s way.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FnPattern {
    tokens: Box<[Token]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Char(char),
    Any,
    Star,
    Set {
        negated: bool,
        items: Vec<(char, char)>,
    },
    Never,
}

impl FnPattern {
    fn new(pattern: &str, sets: SetTranslation) -> Self {
        let pat: Vec<char> = pattern.chars().collect();
        let n = pat.len();
        let mut tokens = Vec::new();
        let mut i = 0;
        while i < n {
            let c = pat[i];
            i += 1;
            match c {
                '*' => {
                    if tokens.last() != Some(&Token::Star) {
                        tokens.push(Token::Star);
                    }
                }
                '?' => tokens.push(Token::Any),
                '[' => {
                    let mut j = i;
                    if j < n && pat[j] == '!' {
                        j += 1;
                    }
                    if j < n && pat[j] == ']' {
                        j += 1;
                    }
                    while j < n && pat[j] != ']' {
                        j += 1;
                    }
                    if j >= n {
                        tokens.push(Token::Char('['));
                        continue;
                    }
                    tokens.push(match sets {
                        SetTranslation::CPython => cpython_set(&pat, i, j),
                        SetTranslation::Legacy => legacy_set(&pat[i..j]),
                    });
                    i = j + 1;
                }
                other => tokens.push(Token::Char(other)),
            }
        }
        Self {
            tokens: tokens.into(),
        }
    }

    /// `fnmatch.fnmatchcase(name, pattern)`: the whole of `name`, `*`
    /// backtracking to the last star.
    fn matches(&self, name: &str) -> bool {
        let name: Vec<char> = name.chars().collect();
        let tokens = &self.tokens;
        let (mut n, mut t) = (0, 0);
        let mut star: Option<(usize, usize)> = None;
        while n < name.len() {
            if t < tokens.len() && tokens[t] == Token::Star {
                star = Some((t, n));
                t += 1;
            } else if t < tokens.len() && one(&tokens[t], name[n]) {
                n += 1;
                t += 1;
            } else if let Some((st, sn)) = star {
                t = st + 1;
                n = sn + 1;
                star = Some((st, sn + 1));
            } else {
                return false;
            }
        }
        tokens[t..].iter().all(|token| *token == Token::Star)
    }
}

/// The set `pat[start..end]` (without its brackets), as `fnmatch` escapes it
/// and `re` then reads it.
fn cpython_set(pat: &[char], start: usize, end: usize) -> Token {
    // `stuff`, with `re` escapes: `\x` is the literal `x`.
    let mut stuff: Vec<(char, bool)> = Vec::new();
    let escape = |chunk: &[char], out: &mut Vec<(char, bool)>| {
        for &c in chunk {
            out.push((c, matches!(c, '\\' | '-' | '&' | '~' | '|')));
        }
    };
    if pat[start..end].contains(&'-') {
        let mut chunks: Vec<Vec<char>> = Vec::new();
        let mut i = start;
        let mut k = if pat[start] == '!' {
            start + 2
        } else {
            start + 1
        };
        loop {
            let found = (k..end).find(|&at| pat[at] == '-');
            let Some(at) = found else { break };
            chunks.push(pat[i..at].to_vec());
            i = at + 1;
            k = at + 3;
        }
        let chunk = pat[i..end].to_vec();
        if chunk.is_empty() {
            if let Some(last) = chunks.last_mut() {
                last.push('-');
            }
        } else {
            chunks.push(chunk);
        }
        // Remove empty ranges, as `fnmatch` does.
        let mut k = chunks.len().saturating_sub(1);
        while k > 0 {
            let (lo, hi) = (chunks[k - 1].last().copied(), chunks[k].first().copied());
            if let (Some(lo), Some(hi)) = (lo, hi)
                && lo > hi
            {
                let tail = chunks[k][1..].to_vec();
                chunks[k - 1].pop();
                chunks[k - 1].extend(tail);
                chunks.remove(k);
            }
            k -= 1;
        }
        for (index, chunk) in chunks.iter().enumerate() {
            if index > 0 {
                stuff.push(('-', false));
            }
            escape(chunk, &mut stuff);
        }
    } else {
        for &c in &pat[start..end] {
            stuff.push((c, matches!(c, '\\' | '&' | '~' | '|')));
        }
    }
    if stuff.is_empty() {
        return Token::Never;
    }
    if stuff == [('!', false)] {
        return Token::Any;
    }
    let negated = stuff[0] == ('!', false);
    let body = if negated { &stuff[1..] } else { &stuff[..] };
    // `re`'s class: an item, then `-` and another item make a range; a `-`
    // with nothing after it is literal.
    let mut items = Vec::new();
    let mut at = 0;
    while at < body.len() {
        let (lo, _) = body[at];
        if body.get(at + 1) == Some(&('-', false)) {
            if let Some(&(hi, _)) = body.get(at + 2) {
                items.push((lo, hi));
                at += 3;
                continue;
            }
            items.push((lo, lo));
            items.push(('-', '-'));
            at += 2;
            continue;
        }
        items.push((lo, lo));
        at += 1;
    }
    Token::Set { negated, items }
}

/// The set `stuff` (without its brackets) read the legacy way.
fn legacy_set(mut stuff: &[char]) -> Token {
    let negated = stuff.first() == Some(&'!');
    if negated {
        stuff = &stuff[1..];
    }
    let mut items = Vec::new();
    let mut k = 0;
    while k < stuff.len() {
        if k + 2 < stuff.len() && stuff[k + 1] == '-' {
            // An empty range (`z-a`) matches nothing.
            if stuff[k] <= stuff[k + 2] {
                items.push((stuff[k], stuff[k + 2]));
            }
            k += 3;
        } else {
            items.push((stuff[k], stuff[k]));
            k += 1;
        }
    }
    Token::Set { negated, items }
}

fn one(token: &Token, c: char) -> bool {
    match token {
        Token::Char(want) => *want == c,
        Token::Any => true,
        Token::Set { negated, items } => {
            items.iter().any(|&(lo, hi)| lo <= c && c <= hi) != *negated
        }
        Token::Never | Token::Star => false,
    }
}

#[cfg(test)]
mod tests;
