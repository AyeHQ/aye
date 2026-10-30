//! Python's `fnmatch.fnmatch`, as unittest's discovery matches file names
//! against its pattern, and its `VALID_MODULE_NAME`.
//!
//! The pattern is translated as `fnmatch._translate` does (Python 3.10 to
//! 3.13 agree), sets included: hyphens between chunks make ranges, empty
//! ranges are dropped, an empty set matches nothing and `[!]` matches any
//! character. The set is then read as `re` reads a character class.

/// `fnmatch.fnmatch(name, pattern)`. Both go through `os.path.normcase`
/// first, which on Windows folds case and turns `/` into `\`.
pub fn fnmatch(name: &str, pattern: &str) -> bool {
    if cfg!(windows) {
        let normcase = |s: &str| s.replace('/', "\\").to_lowercase();
        let name: Vec<char> = normcase(name).chars().collect();
        return matches(&name, &compile(&normcase(pattern)));
    }
    let name: Vec<char> = name.chars().collect();
    matches(&name, &compile(pattern))
}

/// unittest's `VALID_MODULE_NAME`, `[_a-z]\w*\.py$` with `re.IGNORECASE`:
/// the files discovery may import. Under `IGNORECASE`, `[a-z]` also takes
/// `İ`, `ı`, `ſ` and the Kelvin sign; `\w` is a letter, a digit or `_`.
pub fn valid_module_name(name: &str) -> bool {
    let Some(stem) = name
        .len()
        .checked_sub(3)
        .filter(|&at| name.is_char_boundary(at) && name[at..].eq_ignore_ascii_case(".py"))
        .map(|at| &name[..at])
    else {
        return false;
    };
    let mut chars = stem.chars();
    let first_ok = chars.next().is_some_and(|c| {
        c == '_'
            || c.is_ascii_alphabetic()
            || matches!(c, '\u{130}' | '\u{131}' | '\u{17f}' | '\u{212a}')
    });
    first_ok && chars.all(|c| c == '_' || c.is_alphanumeric())
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

fn compile(pattern: &str) -> Vec<Token> {
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
                tokens.push(set(&pat, i, j));
                i = j + 1;
            }
            other => tokens.push(Token::Char(other)),
        }
    }
    tokens
}

/// The set `pat[i..j]` (without its brackets), as `fnmatch` escapes it and
/// `re` then reads it.
fn set(pat: &[char], start: usize, end: usize) -> Token {
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

/// Whole-string match, `*` backtracking to the last star.
fn matches(name: &[char], tokens: &[Token]) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_match_as_python_fnmatch() {
        // Each checked with `fnmatch.fnmatch` on Python 3.13; `normcase`
        // folds case on Windows only.
        let cases = [
            ("test_x.py", "test*.py", true),
            ("test.py", "test*.py", true),
            ("Test_x.py", "test*.py", cfg!(windows)),
            ("test_x.py", "TEST*.PY", cfg!(windows)),
            ("test_x.pyc", "test*.py", false),
            ("x_test.py", "*_test.py", true),
            ("a.py", "?.py", true),
            ("ab.py", "?.py", false),
            ("b.py", "[a-c].py", true),
            ("d.py", "[a-c].py", false),
            ("d.py", "[!a-c].py", true),
            ("-.py", "[a-].py", true),
            ("].py", "[]].py", true),
            ("[.py", "[.py", true),
            ("x.py", "[z-a].py", false),
            ("x.py", "[!].py", false),
            ("!.py", "[!].py", false),
            ("[!].py", "[!].py", true),
            ("q.py", "[!z-a].py", true),
            ("-.py", "[a-c-e].py", true),
            ("d.py", "[a-c-e].py", false),
            ("e.py", "[a-c-e].py", true),
            ("test_a_b.py", "test_*_*.py", true),
            ("", "*", true),
            ("abc", "a*c*", true),
        ];
        for (name, pattern, want) in cases {
            assert_eq!(fnmatch(name, pattern), want, "{name} ~ {pattern}");
        }
    }

    #[test]
    fn module_names_follow_valid_module_name() {
        for name in ["test_x.py", "_x.py", "T.PY", "ſx.py", "té.py", "x1.py"] {
            assert!(valid_module_name(name), "{name}");
        }
        for name in ["1x.py", "t-x.py", "t.x.py", ".py", "x.pyc", "x.p", "éx.py"] {
            assert!(!valid_module_name(name), "{name}");
        }
    }
}
