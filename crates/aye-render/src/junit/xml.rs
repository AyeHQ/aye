//! XML as pytest writes it: escaped text and attributes, elements,
//! properties and captured output's sections.

use std::fmt::Write as _;

/// A `<properties>` element holding `pairs`, when there are some.
pub(super) fn properties(out: &mut String, indent: &str, pairs: &[(String, String)]) {
    if pairs.is_empty() {
        return;
    }
    let _ = writeln!(out, "{indent}<properties>");
    for (name, value) in pairs {
        let _ = writeln!(
            out,
            "{indent}  <property name=\"{}\" value=\"{}\"/>",
            attr(name),
            attr(value)
        );
    }
    let _ = writeln!(out, "{indent}</properties>");
}

/// One stream of captured output as pytest writes it: a header centred in
/// 80 dashes (Python's `str.center`, which, for an even width, puts an odd
/// margin's extra dash on the right), then the text, each on its own line.
pub(super) fn section(header: &str, content: &str) -> String {
    let header = format!(" {header} ");
    let margin = 80usize.saturating_sub(header.chars().count());
    let left = margin / 2;
    format!(
        "{}{header}{}\n{content}\n",
        "-".repeat(left),
        "-".repeat(margin - left)
    )
}

/// One `<failure>`, `<error>` or `<skipped>` element.
pub(super) fn element(
    out: &mut String,
    tag: &str,
    message: Option<&str>,
    kind: Option<&str>,
    text: &str,
) {
    let _ = write!(out, "      <{tag}");
    if let Some(message) = message {
        let _ = write!(out, " message=\"{}\"", attr(message));
    }
    if let Some(kind) = kind {
        let _ = write!(out, " type=\"{}\"", attr(kind));
    }
    if text.is_empty() {
        out.push_str("/>\n");
    } else {
        let _ = writeln!(out, ">{}</{tag}>", body(text));
    }
}

pub(super) fn attr(value: &str) -> String {
    escape(value, true)
}

pub(super) fn body(value: &str) -> String {
    escape(value, false)
}

/// Escapes markup, and shows characters XML 1.0 cannot carry as `#xNN`, as
/// pytest does. Inside attributes, line breaks and tabs are kept as
/// character references, which parsers would otherwise turn into spaces.
fn escape(value: &str, attribute: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            '"' if attribute => out.push_str("&quot;"),
            '\'' if attribute => out.push_str("&apos;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\t' if attribute => out.push_str("&#9;"),
            '\t' | '\n' | ' '..='~' | '\u{80}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}' => {
                out.push(c);
            }
            _ => {
                let code = u32::from(c);
                let _ = if code <= 0xff {
                    write!(out, "#x{code:02X}")
                } else {
                    write!(out, "#x{code:04X}")
                };
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_keeps_the_document_well_formed() {
        assert_eq!(
            attr("a<b & \"c\" 'd'\nx\ty"),
            "a&lt;b &amp; &quot;c&quot; &apos;d&apos;&#10;x&#9;y"
        );
        assert_eq!(body("a<b>\n\"q\"\r"), "a&lt;b&gt;\n\"q\"&#13;");
        assert_eq!(
            body("\u{1b}[31mred\u{7f}\u{0}\u{fffe}é😀"),
            "#x1B[31mred#x7F#x00#xFFFEé😀"
        );
    }

    #[test]
    fn sections_are_headed_as_pytest_heads_them() {
        // `" Captured Out ".center(80, "-")`, then its text.
        let out = section("Captured Out", "noise\n");
        let (header, rest) = out.split_once('\n').unwrap();
        assert_eq!(
            header,
            format!("{} Captured Out {}", "-".repeat(33), "-".repeat(33))
        );
        assert_eq!(rest, "noise\n\n");
        // Python's rule for an odd margin: the extra dash goes right.
        let odd = section("Odd", "");
        assert_eq!(
            odd.lines().next().unwrap(),
            format!("{} Odd {}", "-".repeat(37), "-".repeat(38))
        );
    }
}
