//! A small INI reader matching what pytest accepts via `iniconfig` (2.x, as
//! pytest calls it: inline comments kept in values, section names taken as
//! written).
//!
//! Supports `[section]` headers (a `#` or `;` comment may follow the `]`),
//! `key = value` and `key: value` pairs, indented continuation lines, and
//! `#`/`;` comment lines. Like `iniconfig`, it rejects a key before any
//! section, an empty section name, and a section or a key given twice.

use std::collections::BTreeMap;

/// One INI section: keys to raw values (continuation lines joined with `\n`).
pub type Section = BTreeMap<String, String>;

/// The characters that start a comment, in the order `iniconfig` cuts them
/// from a section header.
const COMMENT_CHARS: [char; 2] = ['#', ';'];

/// Parse INI text into sections.
pub fn parse(source: &str) -> Result<BTreeMap<String, Section>, String> {
    let mut sections: BTreeMap<String, Section> = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut last_key: Option<String> = None;

    for (index, raw) in source
        .strip_prefix('\u{feff}')
        .unwrap_or(source)
        .lines()
        .enumerate()
    {
        let lineno = index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with(COMMENT_CHARS) {
            continue;
        }
        let line = raw.trim_end();

        // A header is a line starting with `[` that, with any comment cut
        // off, ends with `]`. Any other line starting with `[` continues the
        // previous value, as in `iniconfig`.
        let continuation = if line.starts_with([' ', '\t']) {
            Some(trimmed)
        } else if line.starts_with('[') {
            let header = COMMENT_CHARS.iter().fold(line, |text, c| {
                text.split(*c).next().unwrap_or("").trim_end()
            });
            if let Some(name) = header.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
                if name.is_empty() {
                    return Err(format!("line {lineno}: empty section name"));
                }
                if sections.contains_key(name) {
                    return Err(format!("line {lineno}: duplicate section {name:?}"));
                }
                sections.insert(name.to_string(), Section::new());
                current = Some(name.to_string());
                last_key = None;
                continue;
            }
            Some(trimmed)
        } else {
            None
        };

        if let Some(text) = continuation {
            let (Some(section), Some(key)) = (&current, &last_key) else {
                return Err(format!("line {lineno}: unexpected continuation line"));
            };
            let value = sections
                .get_mut(section)
                .and_then(|s| s.get_mut(key))
                .expect("continuation follows a key");
            if !value.is_empty() {
                value.push('\n');
            }
            value.push_str(text);
            continue;
        }

        let Some(section) = &current else {
            return Err(format!("line {lineno}: key outside of a section"));
        };
        let split = trimmed
            .find(['=', ':'])
            .ok_or_else(|| format!("line {lineno}: expected `key = value`"))?;
        let key = trimmed[..split].trim().to_string();
        let value = trimmed[split + 1..].trim().to_string();
        let section = sections.get_mut(section).expect("section exists");
        if section.contains_key(&key) {
            return Err(format!("line {lineno}: duplicate name {key:?}"));
        }
        section.insert(key.clone(), value);
        last_key = Some(key);
    }

    Ok(sections)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sections_and_continuations() {
        let ini = parse(
            "[pytest]\naddopts = -ra\nmarkers =\n    slow: slow tests\n    db\n# comment\n[other]\nx: 1\n",
        )
        .unwrap();
        assert_eq!(ini["pytest"]["addopts"], "-ra");
        assert_eq!(ini["pytest"]["markers"], "slow: slow tests\ndb");
        assert_eq!(ini["other"]["x"], "1");
    }

    #[test]
    fn a_comment_may_follow_a_section_header() {
        for header in [
            "[pytest] # main config",
            "[pytest]  ; main",
            "[pytest]#x",
            "[pytest] ; a # b",
        ] {
            let ini = parse(&format!("{header}\ntestpaths = tests\n")).unwrap();
            assert_eq!(ini["pytest"]["testpaths"], "tests", "{header}");
        }
    }

    #[test]
    fn values_keep_inline_comments() {
        let ini = parse("[pytest]\naddopts = -ra # not a comment\n").unwrap();
        assert_eq!(ini["pytest"]["addopts"], "-ra # not a comment");
    }

    #[test]
    fn section_names_are_taken_as_written() {
        let ini = parse("[ pytest ]\nx = 1\n").unwrap();
        assert!(!ini.contains_key("pytest"));
        assert_eq!(ini[" pytest "]["x"], "1");
    }

    #[test]
    fn a_bracketed_line_that_is_no_header_continues_the_value() {
        let ini = parse("[pytest]\nmarkers =\n[a] b\n").unwrap();
        assert_eq!(ini["pytest"]["markers"], "[a] b");
        let err = parse("[pytest]\n[a] b\n").unwrap_err();
        assert!(err.contains("line 2: unexpected continuation"), "{err}");
    }

    #[test]
    fn what_iniconfig_rejects_is_rejected() {
        let cases = [
            ("x = 1\n", "line 1: key outside of a section"),
            ("[]\n", "line 1: empty section name"),
            ("[a]\n[a]\n", "line 2: duplicate section"),
            ("[a]\nx = 1\nx = 2\n", "line 3: duplicate name"),
            ("[a]\nnothing\n", "line 2: expected `key = value`"),
        ];
        for (ini, expected) in cases {
            let err = parse(ini).unwrap_err();
            assert!(err.contains(expected), "{ini:?}: {err}");
        }
    }

    #[test]
    fn a_byte_order_mark_is_skipped() {
        let ini = parse("\u{feff}[pytest]\nx = 1\n").unwrap();
        assert_eq!(ini["pytest"]["x"], "1");
    }
}
