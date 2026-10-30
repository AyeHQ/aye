//! The `::error` workflow command, escaped as the Actions toolkit escapes
//! it ([`command`]).

/// `::error file=…,line=…,title=…::message`.
pub(super) fn command(file: Option<&str>, line: Option<u32>, title: &str, message: &str) -> String {
    let mut properties = Vec::new();
    if let Some(file) = file {
        properties.push(format!("file={}", property(file)));
        if let Some(line) = line.filter(|line| *line > 0) {
            properties.push(format!("line={line}"));
        }
    }
    properties.push(format!("title={}", property(title)));
    format!("::error {}::{}", properties.join(","), data(message))
}

fn data(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn property(value: &str) -> String {
    data(value).replace(':', "%3A").replace(',', "%2C")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_escape_like_the_actions_toolkit() {
        assert_eq!(
            command(
                Some("tests/a,b.py"),
                Some(3),
                "tests/a,b.py::test_x[50%]",
                "100% broken\r\nsecond: line"
            ),
            "::error file=tests/a%2Cb.py,line=3,title=tests/a%2Cb.py%3A%3Atest_x[50%25]::100%25 broken%0D%0Asecond: line"
        );
        assert_eq!(command(None, Some(3), "aye", "m"), "::error title=aye::m");
        assert_eq!(
            command(Some("t.py"), Some(0), "x", "m"),
            "::error file=t.py,title=x::m"
        );
    }
}
