//! What `doctest.DocTestParser` makes of a text, without Python: how many
//! examples it holds and whether parsing it raises.
//!
//! The parser is regular-expression based (`_EXAMPLE_RE`, `_INDENT_RE`,
//! `_OPTION_DIRECTIVE_RE`, `_IS_BLANK_OR_COMMENT`); each expression is
//! line-shaped, so they are emulated here line by line, on code points, with
//! Python's notion of whitespace. The reference is `Lib/doctest.py` of
//! `CPython` 3.10 to 3.14, which agree on everything read here.

/// The option names `doctest` knows (`OPTIONFLAGS_BY_NAME`), plus the three
/// pytest registers before it parses anything.
pub const KNOWN_OPTIONS: [&str; 16] = [
    "DONT_ACCEPT_TRUE_FOR_1",
    "DONT_ACCEPT_BLANKLINE",
    "NORMALIZE_WHITESPACE",
    "ELLIPSIS",
    "SKIP",
    "IGNORE_EXCEPTION_DETAIL",
    "COMPARISON_FLAGS",
    "REPORT_UDIFF",
    "REPORT_CDIFF",
    "REPORT_NDIFF",
    "REPORT_ONLY_FIRST_FAILURE",
    "REPORTING_FLAGS",
    "FAIL_FAST",
    // pytest's, registered by its doctest plugin.
    "ALLOW_UNICODE",
    "ALLOW_BYTES",
    "NUMBER",
];

/// Whether `name` is an option flag doctest (or pytest) defines.
pub fn known_option(name: &str) -> bool {
    KNOWN_OPTIONS.contains(&name)
}

/// What parsing found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    /// Examples (`>>>` sources that aren't blank or a comment).
    pub examples: usize,
    /// Why `DocTestParser.parse` raises `ValueError`, when it does.
    pub error: Option<String>,
    /// Option names in directives that doctest doesn't define. Parsing
    /// raises unless something registered them at runtime.
    pub unknown_options: Vec<String>,
}

/// `str.isspace()`, which is also what `\s` matches in a `str` pattern.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.expandtabs()` with the default tab size: the column restarts after
/// `\n` and `\r`.
pub fn expand_tabs(text: &str) -> String {
    if !text.contains('\t') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut column = 0usize;
    for c in text.chars() {
        match c {
            '\t' => {
                let spaces = 8 - column % 8;
                out.extend(std::iter::repeat_n(' ', spaces));
                column += spaces;
            }
            '\n' | '\r' => {
                out.push(c);
                column = 0;
            }
            _ => {
                out.push(c);
                column += 1;
            }
        }
    }
    out
}

/// A docstring as Python 3.13 and later store it: the compiler expands its
/// tabs, strips the first line's leading spaces and removes the common
/// indentation of the other lines (`_PyCompile_CleanDoc`).
pub fn clean_doc(doc: &str) -> String {
    let doc = expand_tabs(doc);
    let mut lines = doc.split('\n');
    let first = lines.next().unwrap_or_default();
    let rest: Vec<&str> = lines.collect();
    // The smallest indentation of the lines after the first that hold
    // anything but spaces before their newline.
    // A line of spaces only doesn't count, whether a newline or the end of
    // the string follows them.
    let margin = rest
        .iter()
        .filter_map(|line| {
            let spaces = line.len() - line.trim_start_matches(' ').len();
            (spaces < line.len()).then_some(spaces)
        })
        .min()
        .unwrap_or(0);
    let mut out = String::with_capacity(doc.len());
    out.push_str(first.trim_start_matches(' '));
    for line in rest {
        out.push('\n');
        let spaces = line.len() - line.trim_start_matches(' ').len();
        out.push_str(&line[spaces.min(margin)..]);
    }
    out
}

/// Universal newlines: `\r\n` and `\r` become `\n`, as Python's tokenizer
/// and text files read in text mode give them.
pub fn universal_newlines(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// The leading spaces of `line` (spaces only, as `[ ]*`).
fn indent_of(line: &[char]) -> usize {
    line.iter().take_while(|c| **c == ' ').count()
}

/// `_INDENT_RE` (`^([ ]*)(?=\S)`, multiline): the smallest run of spaces
/// before a non-whitespace character at a line start.
fn min_indent(lines: &[Vec<char>]) -> usize {
    lines
        .iter()
        .filter_map(|line| {
            let spaces = indent_of(line);
            line.get(spaces).filter(|c| !is_space(**c)).map(|_| spaces)
        })
        .min()
        .unwrap_or(0)
}

/// Whether `line` starts with spaces then `prompt`.
fn prompt_line(line: &[char], prompt: &str) -> bool {
    let spaces = indent_of(line);
    let rest = &line[spaces..];
    let prompt: Vec<char> = prompt.chars().collect();
    rest.len() >= prompt.len() && rest[..prompt.len()] == prompt[..]
}

/// Whether a want line may continue the want: not blank (`[ ]*$`) and not a
/// `>>>` line.
fn want_line(line: &[char]) -> bool {
    indent_of(line) < line.len() && !prompt_line(line, ">>>")
}

/// `_IS_BLANK_OR_COMMENT` (`^[ ]*(#.*)?$`, `re.match`, no multiline: `$`
/// also matches before a final newline).
fn blank_or_comment(source: &str) -> bool {
    let rest = source.trim_start_matches(' ');
    if let Some(comment) = rest.strip_prefix('#') {
        match comment.find('\n') {
            None => true,
            Some(at) => at + 1 == comment.len(),
        }
    } else {
        rest.is_empty() || rest == "\n"
    }
}

/// `_OPTION_DIRECTIVE_RE` (`#\s*doctest:\s*([^\n'"]*)$`, multiline): every
/// match's group, as `finditer` finds them.
fn directives(source: &str) -> Vec<String> {
    let chars: Vec<char> = source.chars().collect();
    let n = chars.len();
    let mut found = Vec::new();
    let mut at = 0;
    while at < n {
        if chars[at] != '#' {
            at += 1;
            continue;
        }
        let mut q = at + 1;
        while q < n && is_space(chars[q]) {
            q += 1;
        }
        let keyword: Vec<char> = "doctest:".chars().collect();
        if chars.len() < q + keyword.len() || chars[q..q + keyword.len()] != keyword[..] {
            at += 1;
            continue;
        }
        let start = q + keyword.len();
        let mut spaces_end = start;
        while spaces_end < n && is_space(chars[spaces_end]) {
            spaces_end += 1;
        }
        // `\s*` gives characters back until the group, a greedy run of
        // `[^\n'"]`, ends where `$` holds: at the end or before a newline.
        let mut matched = None;
        for group_start in (start..=spaces_end).rev() {
            let mut end = group_start;
            while end < n && !matches!(chars[end], '\n' | '\'' | '"') {
                end += 1;
            }
            if end == n || chars[end] == '\n' {
                matched = Some((group_start, end));
                break;
            }
        }
        match matched {
            Some((group_start, end)) => {
                found.push(chars[group_start..end].iter().collect());
                at = end.max(at + 1);
            }
            None => at += 1,
        }
    }
    found
}

/// `DocTestParser.parse` (through `get_examples`), on `text` as doctest
/// receives it.
pub fn parse(text: &str) -> Parsed {
    let text = expand_tabs(text);
    let mut lines: Vec<Vec<char>> = text.split('\n').map(|l| l.chars().collect()).collect();
    let strip = min_indent(&lines);
    if strip > 0 {
        for line in &mut lines {
            let cut = strip.min(line.len());
            line.drain(..cut);
        }
    }
    let mut parsed = Parsed::default();
    let n = lines.len();
    let mut i = 0;
    while i < n {
        if !prompt_line(&lines[i], ">>>") {
            i += 1;
            continue;
        }
        // Source: the `>>>` line and every `...` line right after it.
        let mut j = i + 1;
        while j < n && prompt_line(&lines[j], "...") {
            j += 1;
        }
        let source_lines = &lines[i..j];
        // Want: the non-blank lines after it that don't start a new example.
        let mut k = j;
        while k < n && want_line(&lines[k]) {
            k += 1;
        }
        let want_lines = &lines[j..k];
        let indent = indent_of(&lines[i]);
        check_example(source_lines, want_lines, indent, &mut parsed);
        if parsed.error.is_some() {
            return parsed;
        }
        i = k.max(i + 1);
    }
    parsed
}

/// `_parse_example` and the creation of the `Example`.
fn check_example(
    source_lines: &[Vec<char>],
    want_lines: &[Vec<char>],
    indent: usize,
    parsed: &mut Parsed,
) {
    // `_check_prompt_blank`.
    for line in source_lines {
        if line.len() > indent + 3 && line[indent + 3] != ' ' {
            let text: String = line.iter().collect();
            parsed.error = Some(format!("{text:?} lacks a blank after its prompt"));
            return;
        }
    }
    // `_check_prefix` of the `...` lines and of the want.
    let starts_with = |line: &[char], prefix: &[char]| {
        line.is_empty() || (line.len() >= prefix.len() && line[..prefix.len()] == prefix[..])
    };
    let mut dotted: Vec<char> = vec![' '; indent];
    dotted.push('.');
    let spaces: Vec<char> = vec![' '; indent];
    for line in &source_lines[1..] {
        if !starts_with(line, &dotted) {
            let text: String = line.iter().collect();
            parsed.error = Some(format!("{text:?} has inconsistent leading whitespace"));
            return;
        }
    }
    for line in want_lines {
        if !starts_with(line, &spaces) {
            let text: String = line.iter().collect();
            parsed.error = Some(format!("{text:?} has inconsistent leading whitespace"));
            return;
        }
    }
    let source = source_lines
        .iter()
        .map(|line| line.iter().skip(indent + 4).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    // `_find_options`.
    let mut options = 0;
    for group in directives(&source) {
        for option in group
            .replace(',', " ")
            .split(is_space)
            .filter(|o| !o.is_empty())
        {
            let mut chars = option.chars();
            let sign = chars.next();
            let name = chars.as_str();
            if !matches!(sign, Some('+' | '-')) {
                parsed.error = Some(format!("invalid option {option:?}"));
                return;
            }
            if !known_option(name) {
                parsed.unknown_options.push(name.to_string());
            }
            options += 1;
        }
    }
    let blank = blank_or_comment(&source);
    if options > 0 && blank {
        parsed.error = Some(format!(
            "an option directive on a line with no example: {source:?}"
        ));
        return;
    }
    if !blank {
        parsed.examples += 1;
    }
}

/// A docstring's examples on every supported Python: as written (3.10 to
/// 3.12) and as the 3.13 compiler cleans it. Parsing either may raise, and
/// they might disagree on whether there are examples.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Docstring {
    pub examples: bool,
    /// Why the docstring's examples can't be known without Python.
    pub unsure: Option<String>,
}

/// [`Docstring`] of a docstring's value (universal newlines applied).
pub fn docstring(value: &str) -> Docstring {
    if !value.contains(">>>") {
        return Docstring::default();
    }
    let raw = parse(value);
    let clean = parse(&clean_doc(value));
    let unsure = [&raw, &clean]
        .into_iter()
        .find_map(|parsed| {
            parsed.error.clone().or_else(|| {
                parsed.unknown_options.first().map(|name| {
                    format!("option {name:?} is not one doctest defines (registered at runtime?)")
                })
            })
        })
        .or_else(|| {
            ((raw.examples > 0) != (clean.examples > 0))
                .then(|| "its examples depend on the Python version".to_string())
        });
    Docstring {
        examples: raw.examples > 0 || clean.examples > 0,
        unsure,
    }
}

/// [`Docstring`] of a text file's content (universal newlines applied).
pub fn text_file(text: &str) -> Docstring {
    if !text.contains(">>>") {
        return Docstring::default();
    }
    let parsed = parse(text);
    let unsure = parsed.error.clone().or_else(|| {
        parsed.unknown_options.first().map(|name| {
            format!("option {name:?} is not one doctest defines (registered at runtime?)")
        })
    });
    Docstring {
        examples: parsed.examples > 0,
        unsure,
    }
}
