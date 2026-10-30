//! Python's `fnmatch.fnmatch` and pytest's `fnmatch_ex`, which match
//! `--doctest-glob`, `norecursedirs` and `--ignore-glob` patterns.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// `*`: any run of characters, separators included.
    Star,
    /// `?`: one character.
    One,
    /// `[...]` or `[!...]`.
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
    Literal(char),
}

/// `fnmatch.translate`, as tokens.
fn tokens(pattern: &[char]) -> Vec<Token> {
    let mut out = Vec::new();
    let len = pattern.len();
    let mut at = 0;
    while at < len {
        let current = pattern[at];
        at += 1;
        match current {
            '*' => {
                if out.last() != Some(&Token::Star) {
                    out.push(Token::Star);
                }
            }
            '?' => out.push(Token::One),
            '[' => {
                let mut end = at;
                if end < len && pattern[end] == '!' {
                    end += 1;
                }
                if end < len && pattern[end] == ']' {
                    end += 1;
                }
                while end < len && pattern[end] != ']' {
                    end += 1;
                }
                if end >= len {
                    out.push(Token::Literal('['));
                    continue;
                }
                let mut stuff = &pattern[at..end];
                at = end + 1;
                let negated = stuff.first() == Some(&'!');
                if negated {
                    stuff = &stuff[1..];
                }
                let mut ranges = Vec::new();
                let mut item = 0;
                while item < stuff.len() {
                    if item + 2 < stuff.len() && stuff[item + 1] == '-' {
                        let (lo, hi) = (stuff[item], stuff[item + 2]);
                        // Python drops reversed ranges.
                        if lo <= hi {
                            ranges.push((lo, hi));
                        }
                        item += 3;
                    } else {
                        ranges.push((stuff[item], stuff[item]));
                        item += 1;
                    }
                }
                out.push(Token::Class { negated, ranges });
            }
            other => out.push(Token::Literal(other)),
        }
    }
    out
}

fn matches(tokens: &[Token], text: &[char]) -> bool {
    match tokens.split_first() {
        None => text.is_empty(),
        Some((Token::Star, rest)) => (0..=text.len()).any(|at| matches(rest, &text[at..])),
        Some((token, rest)) => {
            let Some((c, more)) = text.split_first() else {
                return false;
            };
            let ok = match token {
                Token::One => true,
                Token::Literal(l) => l == c,
                Token::Class { negated, ranges } => {
                    // An empty class never matches; a negated empty one
                    // matches anything.
                    ranges.iter().any(|(lo, hi)| (lo..=hi).contains(&c)) != *negated
                }
                Token::Star => unreachable!(),
            };
            ok && matches(rest, more)
        }
    }
}

/// `os.path.normcase`: Windows compares paths case-insensitively, with one
/// separator.
fn normcase(text: &str) -> String {
    if cfg!(windows) {
        text.to_lowercase().replace('/', "\\")
    } else {
        text.to_string()
    }
}

/// `fnmatch.fnmatch(name, pattern)`.
pub(crate) fn fnmatch(name: &str, pattern: &str) -> bool {
    let name: Vec<char> = normcase(name).chars().collect();
    let pattern: Vec<char> = normcase(pattern).chars().collect();
    matches(&tokens(&pattern), &name)
}

/// pytest's `fnmatch_ex`: a pattern without a separator matches the file
/// name; one with a separator matches the whole path, anywhere below the
/// root when the path is absolute and the pattern isn't.
pub(crate) fn fnmatch_ex(pattern: &str, path: &Path) -> bool {
    let sep = std::path::MAIN_SEPARATOR;
    let mut pattern = pattern.to_string();
    if cfg!(windows) && !pattern.contains(sep) && pattern.contains('/') {
        pattern = pattern.replace('/', "\\");
    }
    if pattern.contains(sep) {
        if path.is_absolute() && !Path::new(&pattern).is_absolute() {
            pattern = format!("*{sep}{pattern}");
        }
        fnmatch(&path.to_string_lossy(), &pattern)
    } else {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        fnmatch(&name, &pattern)
    }
}
