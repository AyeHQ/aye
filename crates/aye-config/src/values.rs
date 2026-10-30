//! The values `[tool.aye]` holds, each a type that can hold only what the
//! setting accepts. Where the command line takes the same value, the two
//! share the type and keep their own words: `-n` reads a [`WorkerCount`]
//! as pytest-xdist does, `[tool.aye] workers` as TOML gives it.

use std::fmt;
use std::str::FromStr;

use aye_events::AdapterName;
use serde::Deserialize;

/// How many workers a run asks for, as `-n` and pytest-xdist's `-n` read
/// it: a number, or `auto` (and pytest-xdist's `logical`) for one per CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerCount {
    Auto,
    /// A number of workers; 0 is one, as `-n 0` is.
    Count(u32),
}

impl WorkerCount {
    /// The number of workers: a count as given, 0 being one; for `Auto`,
    /// the CPUs the process may use, else one.
    pub fn resolve(self) -> u32 {
        match self {
            Self::Count(n) => n.max(1),
            Self::Auto => std::thread::available_parallelism()
                .map_or(1, |n| u32::try_from(n.get()).unwrap_or(1)),
        }
    }
}

/// As it was given: `auto`, or the number.
impl fmt::Display for WorkerCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Count(n) => write!(f, "{n}"),
        }
    }
}

/// `-n`'s words: a number, `auto` or pytest-xdist's `logical`.
impl FromStr for WorkerCount {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" | "logical" => Ok(Self::Auto),
            n => n
                .parse()
                .map(Self::Count)
                .map_err(|_| format!("expected a number or `auto`, got `{n}`")),
        }
    }
}

/// What `[tool.aye] workers` accepts, for its error message.
const WORKERS_EXPECTED: &str = "a number of workers or \"auto\"";

/// `[tool.aye] workers`'s words: a whole number, or `"auto"`. Not
/// `logical`, which only `-n` takes.
impl<'de> Deserialize<'de> for WorkerCount {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = WorkerCount;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(WORKERS_EXPECTED)
            }

            fn visit_u64<E: serde::de::Error>(self, n: u64) -> Result<WorkerCount, E> {
                u32::try_from(n)
                    .map(WorkerCount::Count)
                    .map_err(|_| E::custom(format!("workers = {n}: too many")))
            }

            fn visit_i64<E: serde::de::Error>(self, n: i64) -> Result<WorkerCount, E> {
                u64::try_from(n).map_or_else(
                    |_| {
                        Err(E::custom(format!(
                            "workers = {n}: expected {WORKERS_EXPECTED}"
                        )))
                    },
                    |n| self.visit_u64(n),
                )
            }

            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<WorkerCount, E> {
                if s == "auto" {
                    Ok(WorkerCount::Auto)
                } else {
                    Err(E::custom(format!(
                        "workers = {s:?}: expected {WORKERS_EXPECTED}"
                    )))
                }
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

/// The output formats. How much each shows is the verbosity's (`-v`,
/// `-q`), the command line's alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// For people: quiet on success, readable failures.
    Text,
    /// JSON lines, for programs and coding agents.
    Json,
}

impl OutputFormat {
    /// Every format, as `--output` lists them.
    pub const ALL: [Self; 2] = [Self::Text, Self::Json];

    /// Its name, as `--output` and `[tool.aye] output` take it (in any
    /// case).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
        }
    }

    /// The format named `name`, in any case.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|format| format.as_str().eq_ignore_ascii_case(name))
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `--output`'s words: a format's name in any case. The names the formats
/// had until October 2026 (`human`, `agent`; never published) are refused
/// with what to use instead. `[tool.aye] output` says it in its own words
/// ([`crate::AyeSettings::resolve`]).
impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "human" => Err("`human` is now `text`, the default: use `--output text`".into()),
            "agent" => Err("`agent` is now `json` at `-v`: use `--output json -v`".into()),
            _ => Err(format!("expected `text` or `json`, got `{s}`")),
        }
    }
}

/// A per-test timeout: a finite number of seconds, 0 or more. 0 is none,
/// as pytest-timeout reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timeout(f64);

impl Timeout {
    /// `seconds` as a timeout, unless it is negative or not finite.
    pub fn new(seconds: f64) -> Option<Self> {
        (seconds.is_finite() && seconds >= 0.0).then_some(Self(seconds))
    }

    /// The seconds, as given.
    pub const fn seconds(self) -> f64 {
        self.0
    }

    /// The seconds a test may take, if they are limited: not for 0.
    pub fn per_test(self) -> Option<f64> {
        (self.0 > 0.0).then_some(self.0)
    }
}

/// The seconds, as Rust prints them (`5` for 5.0).
impl fmt::Display for Timeout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// How many runs' logs `.aye/runs` keeps, for `aye show`: 0 keeps none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeepRuns(u64);

impl KeepRuns {
    /// When `keep-runs` isn't set.
    pub const DEFAULT: Self = Self(10);

    /// `runs` as a number to keep, unless it is negative.
    pub fn new(runs: i64) -> Option<Self> {
        u64::try_from(runs).ok().map(Self)
    }

    /// The number of runs to keep; none where it doesn't fit a `usize`.
    pub fn get(self) -> usize {
        usize::try_from(self.0).unwrap_or(0)
    }
}

impl fmt::Display for KeepRuns {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The adapters a run uses, in priority order: at least one, none blank,
/// none twice. Which names exist is the binary's to check (its registry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterList(Vec<AdapterName>);

/// Why names aren't an [`AdapterList`]: the first rule they break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidAdapterList {
    Empty,
    /// A blank name, as written.
    Blank(String),
    /// A name listed a second time.
    Twice(String),
}

impl AdapterList {
    /// `names` in order, checked one by one: a blank name or a repeat is
    /// reported where it first comes.
    pub fn new(names: Vec<String>) -> Result<Self, InvalidAdapterList> {
        if names.is_empty() {
            return Err(InvalidAdapterList::Empty);
        }
        for (at, name) in names.iter().enumerate() {
            if name.trim().is_empty() {
                return Err(InvalidAdapterList::Blank(name.clone()));
            }
            if names[..at].contains(name) {
                return Err(InvalidAdapterList::Twice(name.clone()));
            }
        }
        Ok(Self(names.into_iter().map(AdapterName::from).collect()))
    }

    pub fn names(&self) -> &[AdapterName] {
        &self.0
    }
}

/// The names joined with `, `, as `aye config` shows them.
impl fmt::Display for AdapterList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (at, name) in self.0.iter().enumerate() {
            if at > 0 {
                f.write_str(", ")?;
            }
            f.write_str(name.as_str())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worker_count_is_read_with_each_surfaces_words() {
        assert_eq!("logical".parse(), Ok(WorkerCount::Auto));
        assert_eq!("auto".parse(), Ok(WorkerCount::Auto));
        assert_eq!("3".parse(), Ok(WorkerCount::Count(3)));
        assert_eq!(
            "many".parse::<WorkerCount>(),
            Err("expected a number or `auto`, got `many`".to_string())
        );
        let toml = |value: &str| {
            toml::from_str::<toml::Table>(&format!("workers = {value}")).unwrap()["workers"]
                .clone()
                .try_into::<WorkerCount>()
                .map_err(|err| err.message().to_string())
        };
        assert_eq!(toml("\"auto\""), Ok(WorkerCount::Auto));
        assert_eq!(toml("0"), Ok(WorkerCount::Count(0)));
        assert_eq!(
            toml("\"logical\""),
            Err("workers = \"logical\": expected a number of workers or \"auto\"".to_string())
        );
        assert_eq!(WorkerCount::Auto.to_string(), "auto");
        assert_eq!(WorkerCount::Count(4).to_string(), "4");
        assert_eq!(WorkerCount::Count(0).resolve(), 1);
    }

    #[test]
    fn an_output_format_is_named_in_any_case() {
        for format in OutputFormat::ALL {
            assert_eq!(OutputFormat::named(format.as_str()), Some(format));
            assert_eq!(
                OutputFormat::named(&format.as_str().to_uppercase()),
                Some(format)
            );
        }
        assert_eq!(OutputFormat::named("human"), None);
        // The command line takes every name the configuration does.
        for format in OutputFormat::ALL {
            assert_eq!(format.as_str().parse(), Ok(format));
            assert_eq!(format.as_str().to_uppercase().parse(), Ok(format));
        }
        assert_eq!(
            "Human".parse::<OutputFormat>(),
            Err("`human` is now `text`, the default: use `--output text`".to_string())
        );
        assert_eq!(
            "xml".parse::<OutputFormat>(),
            Err("expected `text` or `json`, got `xml`".to_string())
        );
    }

    #[test]
    fn timeouts_and_kept_runs_hold_what_they_accept() {
        assert_eq!(Timeout::new(0.0).and_then(Timeout::per_test), None);
        assert_eq!(Timeout::new(2.5).and_then(Timeout::per_test), Some(2.5));
        for bad in [-1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(Timeout::new(bad), None, "{bad}");
        }
        assert_eq!(Timeout::new(5.0).unwrap().to_string(), "5");
        assert_eq!(KeepRuns::new(-1), None);
        assert_eq!(KeepRuns::new(0).map(KeepRuns::get), Some(0));
        assert_eq!(KeepRuns::DEFAULT.get(), 10);
    }

    #[test]
    fn an_adapter_list_is_at_least_one_name_none_twice() {
        let list =
            |names: &[&str]| AdapterList::new(names.iter().map(ToString::to_string).collect());
        assert_eq!(
            list(&["unittest", "pytest"]).unwrap().to_string(),
            "unittest, pytest"
        );
        assert_eq!(list(&[]), Err(InvalidAdapterList::Empty));
        assert_eq!(
            list(&["pytest", " ", "pytest"]),
            Err(InvalidAdapterList::Blank(" ".to_string()))
        );
        assert_eq!(
            list(&["pytest", "doctest", "pytest", ""]),
            Err(InvalidAdapterList::Twice("pytest".to_string()))
        );
    }
}
