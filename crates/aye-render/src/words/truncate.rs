//! Text cut to a length: its start ([`head`]), its end ([`tail`]), its
//! last lines ([`last_lines`]).

use std::borrow::Cow;

/// At most `max` characters of `text`; a cut ends in `…`.
pub(crate) fn head(text: &str, max: usize) -> Cow<'_, str> {
    if text.chars().nth(max).is_none() {
        return Cow::Borrowed(text);
    }
    let end = text
        .char_indices()
        .nth(max.saturating_sub(1))
        .map_or(text.len(), |(at, _)| at);
    Cow::Owned(format!("{}…", &text[..end]))
}

/// The last `max` characters of `text`; a cut starts with `…`.
pub(crate) fn tail(text: &str, max: usize) -> Cow<'_, str> {
    let count = text.chars().count();
    if count <= max {
        return Cow::Borrowed(text);
    }
    let start = text
        .char_indices()
        .nth(count - max.saturating_sub(1))
        .map_or(text.len(), |(at, _)| at);
    Cow::Owned(format!("…{}", &text[start..]))
}

/// The end of some captured text.
pub(crate) struct Lines<'a> {
    pub lines: Vec<&'a str>,
    /// How many lines the text has.
    pub total: usize,
}

/// The last `n` lines of `text`, trailing blank lines dropped.
pub(crate) fn last_lines(text: &str, n: usize) -> Lines<'_> {
    let all: Vec<&str> = text.trim_end().lines().collect();
    let total = all.len();
    Lines {
        lines: all[total.saturating_sub(n)..].to_vec(),
        total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_and_tail_cut_on_characters() {
        assert_eq!(head("abcdef", 6), "abcdef");
        assert_eq!(head("abcdefg", 6), "abcde…");
        assert_eq!(head("ééééééé", 3), "éé…");
        assert_eq!(tail("abcdef", 6), "abcdef");
        assert_eq!(tail("abcdefg", 6), "…cdefg");
        assert_eq!(tail("ééééééé", 3), "…éé");
    }

    #[test]
    fn last_lines_counts_every_line() {
        let lines = last_lines("a\nb\nc\nd\n\n", 2);
        assert_eq!(lines.lines, ["c", "d"]);
        assert_eq!(lines.total, 4);
        assert_eq!(last_lines("", 3).total, 0);
    }
}
