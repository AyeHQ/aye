//! Python's `os.path`, as the adapters read paths with it: variables and
//! `~` expanded (`expandvars`, `expanduser`), in `posixpath`'s forms and
//! `ntpath`'s, over a lookup of the environment they are given
//! (`ProcessEnv::get`, from the binary), whichever variable a path names.
//!
//! `expanduser` reads the variables `posixpath` and `ntpath` read, by the
//! names they read them by. This crate can't name aye-static's `EnvVars`,
//! which catalogues them.

/// The home directory, to `posixpath.expanduser`.
const HOME: &str = "HOME";
/// The home directory, to `ntpath.expanduser`.
const USERPROFILE: &str = "USERPROFILE";
/// The home directory's drive where `USERPROFILE` isn't set, to
/// `ntpath.expanduser`.
const HOMEDRIVE: &str = "HOMEDRIVE";
/// The home directory's path where `USERPROFILE` isn't set, to
/// `ntpath.expanduser`.
const HOMEPATH: &str = "HOMEPATH";
/// The user's name, for `~user`, to `ntpath.expanduser`.
const USERNAME: &str = "USERNAME";

/// Python's `os.path.expandvars`: `ntpath`'s on Windows, else `posixpath`'s,
/// over the variables `var` finds, whichever `text` names.
pub fn expandvars(text: &str, var: impl Fn(&str) -> Option<String>) -> String {
    if cfg!(windows) {
        nt_expandvars(text, var)
    } else {
        posix_expandvars(text, var)
    }
}

/// Python's `os.path.expanduser`: `ntpath`'s on Windows, else
/// `posixpath`'s, over the variables `var` finds.
pub fn expanduser(text: &str, var: impl Fn(&str) -> Option<String>) -> String {
    if cfg!(windows) {
        nt_expanduser(text, var)
    } else {
        posix_expanduser(text, var(HOME))
    }
}

/// `posixpath.expandvars`: `$name` and `${name}` replaced by the variable
/// `var` finds, when it finds one.
fn posix_expandvars(text: &str, var: impl Fn(&str) -> Option<String>) -> String {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (name, len) = if let Some(braced) = after.strip_prefix('{') {
            match braced.find('}') {
                Some(end) => (&braced[..end], end + 2),
                None => ("", 0),
            }
        } else {
            let end = after.find(|c: char| !word(c)).unwrap_or(after.len());
            (&after[..end], end)
        };
        match var(name) {
            Some(value) => out.push_str(&value),
            None => out.push_str(&rest[at..=at + len]),
        }
        rest = &rest[at + 1 + len..];
    }
    out.push_str(rest);
    out
}

/// `ntpath.expandvars`: `%name%`, `$name` and `${name}` replaced by the
/// variable `var` finds, when it finds one; `%%` and `$$` become `%` and
/// `$`, and nothing within single quotes is expanded. An unclosed `%`,
/// `${` or `'` leaves the rest as it is.
fn nt_expandvars(text: &str, var: impl Fn(&str) -> Option<String>) -> String {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(['\'', '%', '$']) {
        out.push_str(&rest[..at]);
        let token = &rest[at..];
        let after = &token[1..];
        // How long the token is, and what it becomes, if not itself.
        let (len, value) = if token.starts_with('\'') {
            (after.find('\'').map_or(token.len(), |end| end + 2), None)
        } else if token.starts_with('%') {
            if after.starts_with('%') {
                (2, Some("%".to_string()))
            } else {
                match after.find('%') {
                    Some(end) => (end + 2, var(&after[..end])),
                    None => (token.len(), None),
                }
            }
        } else if after.starts_with('$') {
            (2, Some("$".to_string()))
        } else if let Some(braced) = after.strip_prefix('{') {
            match braced.find('}') {
                Some(end) => (end + 3, var(&braced[..end])),
                None => (token.len(), None),
            }
        } else {
            let end = after.find(|c: char| !word(c)).unwrap_or(after.len());
            (end + 1, if end == 0 { None } else { var(&after[..end]) })
        };
        out.push_str(value.as_deref().unwrap_or(&token[..len]));
        rest = &token[len..];
    }
    out.push_str(rest);
    out
}

/// `posixpath.expanduser` for the current user, whose home is `home`: a
/// leading `~` becomes it. `~user` is left alone.
fn posix_expanduser(text: &str, home: Option<String>) -> String {
    let Some(rest) = text.strip_prefix('~') else {
        return text.to_string();
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        return text.to_string();
    }
    match home {
        Some(home) => {
            let home = home.trim_end_matches('/');
            let joined = format!("{home}{rest}");
            if joined.is_empty() {
                "/".to_string()
            } else {
                joined
            }
        }
        None => text.to_string(),
    }
}

/// `ntpath.expanduser`, with the variables `var` finds: a leading `~`
/// becomes `USERPROFILE` (else `HOMEDRIVE` and `HOMEPATH`); `~user`, that
/// user's home, guessed beside the current user's (`USERNAME`) as Python
/// guesses it.
fn nt_expanduser(text: &str, var: impl Fn(&str) -> Option<String>) -> String {
    let Some(rest) = text.strip_prefix('~') else {
        return text.to_string();
    };
    let (user, tail) = rest.split_at(rest.find(['\\', '/']).unwrap_or(rest.len()));
    let mut home = match (var(USERPROFILE), var(HOMEPATH)) {
        (Some(profile), _) => profile,
        (None, Some(path)) => nt_join(&var(HOMEDRIVE).unwrap_or_default(), &path),
        (None, None) => return text.to_string(),
    };
    if !user.is_empty() {
        let current = var(USERNAME);
        if current.as_deref() != Some(user) {
            let (parent, name) = nt_split(&home);
            if current.as_deref() != Some(name) {
                return text.to_string();
            }
            home = nt_join(parent, user);
        }
    }
    format!("{home}{tail}")
}

/// `ntpath.split` of a home directory: its parent (keeping the root, `C:\`)
/// and its name.
fn nt_split(path: &str) -> (&str, &str) {
    let Some(at) = path.rfind(['\\', '/']) else {
        let drive = if path.as_bytes().get(1) == Some(&b':') {
            2
        } else {
            0
        };
        return path.split_at(drive);
    };
    let head = path[..at].trim_end_matches(['\\', '/']);
    if head.is_empty() || head.ends_with(':') {
        (&path[..=at], &path[at + 1..])
    } else {
        (head, &path[at + 1..])
    }
}

/// `ntpath.join` of a drive or a directory and a name or a rooted path.
fn nt_join(head: &str, tail: &str) -> String {
    if head.is_empty() || head.ends_with(['\\', '/', ':']) || tail.starts_with(['\\', '/']) {
        format!("{head}{tail}")
    } else {
        format!("{head}\\{tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expandvars_follows_python() {
        // Each checked against `posixpath.expandvars` and `ntpath.expandvars`
        // (Python 3.11 and 3.14) with this environment, whose names Windows
        // reads whatever their case.
        let env = [("HOME", "/home/me"), ("MY-VAR", "dash"), ("1x", "num")];
        let posix = |name: &str| {
            let found = env.iter().find(|(key, _)| *key == name);
            found.map(|(_, value)| (*value).to_string())
        };
        let nt = |name: &str| {
            let found = env.iter().find(|(key, _)| key.eq_ignore_ascii_case(name));
            found.map(|(_, value)| (*value).to_string())
        };
        for (text, on_posix, on_nt) in [
            ("$HOME", "/home/me", "/home/me"),
            ("${HOME}/x", "/home/me/x", "/home/me/x"),
            ("a$HOME.b", "a/home/me.b", "a/home/me.b"),
            ("$AYE_UNSET", "$AYE_UNSET", "$AYE_UNSET"),
            ("${AYE_UNSET}", "${AYE_UNSET}", "${AYE_UNSET}"),
            ("${}", "${}", "${}"),
            ("${a=b}", "${a=b}", "${a=b}"),
            ("${unclosed", "${unclosed", "${unclosed"),
            ("${HOME", "${HOME", "${HOME"),
            ("$", "$", "$"),
            ("$$", "$$", "$"),
            ("$$HOME", "$/home/me", "$HOME"),
            ("cost $5", "cost $5", "cost $5"),
            ("$1x", "num", "num"),
            ("$home", "$home", "/home/me"),
            ("$HOME-x", "/home/me-x", "$HOME-x"),
            ("$MY-VAR/x", "$MY-VAR/x", "dash/x"),
            ("${MY-VAR}", "dash", "dash"),
            ("%HOME%", "%HOME%", "/home/me"),
            ("%home%\\x", "%home%\\x", "/home/me\\x"),
            ("%MY-VAR%", "%MY-VAR%", "dash"),
            ("%AYE_UNSET%", "%AYE_UNSET%", "%AYE_UNSET%"),
            ("%=a%", "%=a%", "%=a%"),
            ("%", "%", "%"),
            ("%%", "%%", "%"),
            ("100%", "100%", "100%"),
            ("%HOME", "%HOME", "%HOME"),
            ("a%%b%HOME%", "a%%b%HOME%", "a%b/home/me"),
            ("%%HOME%%", "%%HOME%%", "%HOME%"),
            ("$HOME%HOME%", "/home/me%HOME%", "/home/me/home/me"),
            ("'$HOME'", "'/home/me'", "'$HOME'"),
            ("'%HOME%", "'%HOME%", "'%HOME%"),
            ("it's $HOME", "it's /home/me", "it's $HOME"),
            ("'", "'", "'"),
            ("''$HOME", "''/home/me", "''/home/me"),
            (
                "x%HOME%y'%HOME%'%HOME%",
                "x%HOME%y'%HOME%'%HOME%",
                "x/home/mey'%HOME%'/home/me",
            ),
        ] {
            assert_eq!(posix_expandvars(text, posix), on_posix, "posix: {text}");
            assert_eq!(nt_expandvars(text, nt), on_nt, "nt: {text}");
        }
    }

    #[test]
    fn expanduser_follows_python() {
        type Env<'a> = &'a [(&'a str, &'a str)];
        // Checked against `posixpath.expanduser` with this HOME.
        for (text, home, expected) in [
            ("~", "/home/me", "/home/me"),
            ("~/r.xml", "/home/me/", "/home/me/r.xml"),
            ("~", "/", "/"),
            ("~/x", "/", "/x"),
            ("x~", "/home/me", "x~"),
            ("~aye-no-such-user/x", "/home/me", "~aye-no-such-user/x"),
        ] {
            assert_eq!(
                posix_expanduser(text, Some(home.to_string())),
                expected,
                "{text}"
            );
        }
        // Checked against `ntpath.expanduser` with each environment.
        let texts = [
            "~", "~/r.xml", "~\\r.xml", "~me\\x", "~other/x", "~other", "x~", "~me",
        ];
        let cases: [(Env<'_>, [&str; 8]); 6] = [
            (
                &[("USERPROFILE", "C:\\Users\\me"), ("USERNAME", "me")],
                [
                    "C:\\Users\\me",
                    "C:\\Users\\me/r.xml",
                    "C:\\Users\\me\\r.xml",
                    "C:\\Users\\me\\x",
                    "C:\\Users\\other/x",
                    "C:\\Users\\other",
                    "x~",
                    "C:\\Users\\me",
                ],
            ),
            (
                &[("USERPROFILE", "C:\\Users\\me")],
                [
                    "C:\\Users\\me",
                    "C:\\Users\\me/r.xml",
                    "C:\\Users\\me\\r.xml",
                    "~me\\x",
                    "~other/x",
                    "~other",
                    "x~",
                    "~me",
                ],
            ),
            (
                &[
                    ("HOMEDRIVE", "C:"),
                    ("HOMEPATH", "\\Users\\me"),
                    ("USERNAME", "me"),
                ],
                [
                    "C:\\Users\\me",
                    "C:\\Users\\me/r.xml",
                    "C:\\Users\\me\\r.xml",
                    "C:\\Users\\me\\x",
                    "C:\\Users\\other/x",
                    "C:\\Users\\other",
                    "x~",
                    "C:\\Users\\me",
                ],
            ),
            (
                &[("HOMEPATH", "\\Users\\me"), ("USERNAME", "me")],
                [
                    "\\Users\\me",
                    "\\Users\\me/r.xml",
                    "\\Users\\me\\r.xml",
                    "\\Users\\me\\x",
                    "\\Users\\other/x",
                    "\\Users\\other",
                    "x~",
                    "\\Users\\me",
                ],
            ),
            (
                &[("USERPROFILE", "C:\\me"), ("USERNAME", "me")],
                [
                    "C:\\me",
                    "C:\\me/r.xml",
                    "C:\\me\\r.xml",
                    "C:\\me\\x",
                    "C:\\other/x",
                    "C:\\other",
                    "x~",
                    "C:\\me",
                ],
            ),
            (&[("USERNAME", "me")], texts),
        ];
        for (env, expected) in cases {
            let var = |name: &str| {
                let found = env.iter().find(|(key, _)| *key == name);
                found.map(|(_, value)| (*value).to_string())
            };
            for (text, expected) in texts.iter().zip(expected) {
                assert_eq!(nt_expanduser(text, var), expected, "{text} with {env:?}");
            }
        }
    }
}
