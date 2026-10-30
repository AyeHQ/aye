//! `[tool.aye]` resolved ([`AyeSettings`]): every value it sets, typed,
//! with where it came from.

use std::path::{Path, PathBuf};

use crate::ConfigError;
use crate::options::AyeOptions;
use crate::source::{Sourced, ValueSource};
use crate::values::{
    AdapterList, InvalidAdapterList, KeepRuns, OutputFormat, Timeout, WorkerCount,
};

/// `[tool.aye]`, resolved: each value it sets, of a type that holds only
/// what the setting accepts, with the table as its source. A value it
/// doesn't set is `None`; what the run uses then is the command line's or
/// a default, which the methods below say where it is the table's to say.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AyeSettings {
    /// Worker processes.
    pub workers: Option<Sourced<WorkerCount>>,
    /// The interpreter that runs the tests: a path, or a name looked up on
    /// `PATH`; never blank.
    pub python: Option<Sourced<PathBuf>>,
    /// The output format. How much it shows is the command line's (`-v`,
    /// `-q`).
    pub output: Option<Sourced<OutputFormat>>,
    /// The per-test timeout (see [`AyeSettings::timeout`]).
    pub timeout: Option<Sourced<Timeout>>,
    /// Which adapters run, in priority order. Unset: chosen from the
    /// project (see the binary's registry).
    pub adapters: Option<Sourced<AdapterList>>,
    /// How many runs' logs `.aye/runs` keeps (see
    /// [`AyeSettings::keep_runs`]).
    pub keep_runs: Option<Sourced<KeepRuns>>,
    /// `[tool.aye.unittest]`: the unittest adapter's own settings, read by
    /// that adapter.
    pub unittest: Option<toml::Table>,
    /// `[tool.aye.doctest]`: the doctest adapter's own settings, read by
    /// that adapter.
    pub doctest: Option<toml::Table>,
}

/// The names the output formats had until October 2026 (never published),
/// with what replaces each, for the error that rejects them.
const RENAMED_OUTPUTS: [(&str, &str); 2] = [
    ("human", "\"text\""),
    (
        "agent",
        "\"json\" at -v (output = \"json\", and run aye -v)",
    ),
];

impl AyeSettings {
    /// No `[tool.aye]`: nothing set, as [`AyeSettings::default`].
    pub const NONE: Self = Self {
        workers: None,
        python: None,
        output: None,
        timeout: None,
        adapters: None,
        keep_runs: None,
        unittest: None,
        doctest: None,
    };

    /// `[tool.aye]` from `root/pyproject.toml`, or [`AyeSettings::NONE`]
    /// when there is none.
    pub fn load(root: &Path) -> Result<Self, ConfigError> {
        let path = root.join("pyproject.toml");
        if !path.is_file() {
            return Ok(Self::NONE);
        }
        let Some(options) = AyeOptions::read(&path)? else {
            return Ok(Self::NONE);
        };
        let source = ValueSource::file(&path, "[tool.aye]");
        Self::resolve(options, &source).map_err(|message| ConfigError::Aye { path, message })
    }

    /// The values `options` sets, each from `source`, once each is one its
    /// setting accepts: `output` is `text` or `json` in any case, `python`
    /// isn't blank, `timeout` is a finite number of seconds, 0 or more,
    /// `keep-runs` a whole number, 0 or more, and `adapters` lists at least
    /// one name, none blank and none twice. They are checked in that order,
    /// and the error names the first bad one.
    pub fn resolve(options: AyeOptions, source: &ValueSource) -> Result<Self, String> {
        let AyeOptions {
            workers,
            python,
            output,
            timeout,
            adapters,
            keep_runs,
            unittest,
            doctest,
        } = options;
        let output = match output {
            None => None,
            Some(output) => {
                let Some(format) = OutputFormat::named(&output) else {
                    let instead = RENAMED_OUTPUTS
                        .iter()
                        .find(|(old, _)| old.eq_ignore_ascii_case(&output))
                        .map_or_else(String::new, |(old, new)| format!("; {old:?} is now {new}"));
                    return Err(format!(
                        "output = {output:?}: expected text or json{instead}"
                    ));
                };
                Some(Sourced::new(format, source.clone()))
            }
        };
        if let Some(python) = &python
            && python.trim().is_empty()
        {
            return Err(format!(
                "python = {python:?}: expected the path or name of an interpreter"
            ));
        }
        let timeout = match timeout {
            None => None,
            Some(seconds) => match Timeout::new(seconds) {
                Some(timeout) => Some(Sourced::new(timeout, source.clone())),
                None => {
                    return Err(format!(
                        "timeout = {seconds}: expected a number of seconds, 0 or more"
                    ));
                }
            },
        };
        let keep_runs = match keep_runs {
            None => None,
            Some(keep) => match KeepRuns::new(keep) {
                Some(keep) => Some(Sourced::new(keep, source.clone())),
                None => {
                    return Err(format!(
                        "keep-runs = {keep}: expected a number of runs, 0 or more"
                    ));
                }
            },
        };
        let adapters = match adapters.map(AdapterList::new).transpose() {
            Ok(adapters) => adapters.map(|adapters| Sourced::new(adapters, source.clone())),
            Err(InvalidAdapterList::Empty) => {
                return Err("adapters = []: list at least one adapter".to_string());
            }
            Err(InvalidAdapterList::Blank(name)) => {
                return Err(format!("adapters: {name:?} is not an adapter's name"));
            }
            Err(InvalidAdapterList::Twice(name)) => {
                return Err(format!("adapters: {name:?} is listed twice"));
            }
        };
        Ok(Self {
            workers: workers.map(|workers| Sourced::new(workers, source.clone())),
            python: python.map(|python| Sourced::new(PathBuf::from(python), source.clone())),
            output,
            timeout,
            adapters,
            keep_runs,
            unittest,
            doctest,
        })
    }

    /// The per-test timeout in seconds, if any: `timeout = 0` is none, as
    /// pytest-timeout reads it.
    pub fn timeout(&self) -> Option<f64> {
        self.timeout.as_ref()?.value.per_test()
    }

    /// How many runs' logs to keep: `keep-runs`, else
    /// [`KeepRuns::DEFAULT`]. 0 keeps none.
    pub fn keep_runs(&self) -> usize {
        self.keep_runs
            .as_ref()
            .map_or(KeepRuns::DEFAULT, |keep| keep.value)
            .get()
    }

    /// An adapter's own table, `[tool.aye.<name>]`, when it is set.
    pub fn adapter_table(&self, name: &str) -> Option<&toml::Table> {
        match name {
            "unittest" => self.unittest.as_ref(),
            "doctest" => self.doctest.as_ref(),
            _ => None,
        }
    }

    /// Every setting that is set, as `(name, value, source)`, for `aye
    /// config`.
    pub fn entries(&self) -> Vec<(&'static str, String, &ValueSource)> {
        fn entry<'a, T>(
            name: &'static str,
            value: Option<&'a Sourced<T>>,
            shown: impl Fn(&T) -> String,
        ) -> Option<(&'static str, String, &'a ValueSource)> {
            value.map(|value| (name, shown(&value.value), &value.source))
        }
        [
            entry("workers", self.workers.as_ref(), ToString::to_string),
            entry("python", self.python.as_ref(), |python| {
                python.display().to_string()
            }),
            entry("output", self.output.as_ref(), ToString::to_string),
            entry("timeout", self.timeout.as_ref(), ToString::to_string),
            entry("adapters", self.adapters.as_ref(), ToString::to_string),
            entry("keep-runs", self.keep_runs.as_ref(), ToString::to_string),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn write(root: &Path, rel: &str, body: &str) -> PathBuf {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn tool_aye_with_its_source_and_strict_keys() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "pyproject.toml",
            "[tool.aye]\nworkers = \"auto\"\ntimeout = 5.0\n",
        );
        let config = AyeSettings::load(dir.path()).unwrap();
        assert_eq!(
            config.workers.as_ref().map(|w| w.value),
            Some(WorkerCount::Auto)
        );
        let entries = config.entries();
        assert_eq!(entries[0].0, "workers");
        assert_eq!(
            entries[0].2.display_row(dir.path()).to_string(),
            "pyproject.toml [tool.aye]"
        );

        write(dir.path(), "pyproject.toml", "[tool.aye]\nwrokers = 4\n");
        let err = AyeSettings::load(dir.path()).unwrap_err();
        assert!(err.to_string().contains("wrokers"), "{err}");
    }

    /// Finding 82: a bad `output` was ignored, a bad `workers` named serde's
    /// untagged enum.
    #[test]
    fn tool_aye_values_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let load = |body: &str| {
            write(
                dir.path(),
                "pyproject.toml",
                &format!("[tool.aye]\n{body}\n"),
            );
            AyeSettings::load(dir.path())
        };
        for output in ["text", "json"] {
            let config = load(&format!("output = {output:?}")).unwrap();
            assert_eq!(config.output.map(|o| o.value.as_str()), Some(output));
        }
        // Any case, as `--output` takes it, kept in lower case.
        for (output, kept) in [("JSON", "json"), ("Text", "text"), ("tEXT", "text")] {
            let config = load(&format!("output = {output:?}")).unwrap();
            assert_eq!(config.output.as_ref().map(|o| o.value.as_str()), Some(kept));
            let entries = config.entries();
            assert_eq!(
                entries[0],
                (
                    "output",
                    kept.to_string(),
                    &ValueSource::file(dir.path().join("pyproject.toml"), "[tool.aye]")
                )
            );
        }
        let err = load("output = \"xml\"").unwrap_err().to_string();
        assert!(
            err.ends_with("output = \"xml\": expected text or json"),
            "{err}"
        );
        assert!(err.contains("pyproject.toml"), "{err}");
        // The formats' old names say what to use instead, in any case.
        for (output, says) in [
            (
                "human",
                "output = \"human\": expected text or json; \"human\" is now \"text\"",
            ),
            (
                "Human",
                "output = \"Human\": expected text or json; \"human\" is now \"text\"",
            ),
            (
                "agent",
                "output = \"agent\": expected text or json; \"agent\" is now \"json\" at -v \
                 (output = \"json\", and run aye -v)",
            ),
            (
                "AGENT",
                "output = \"AGENT\": expected text or json; \"agent\" is now \"json\" at -v \
                 (output = \"json\", and run aye -v)",
            ),
        ] {
            let err = load(&format!("output = {output:?}"))
                .unwrap_err()
                .to_string();
            assert!(err.ends_with(says), "{err}");
        }

        let workers = |body: &str| load(body).unwrap().workers.map(|w| w.value);
        assert_eq!(workers("workers = 4"), Some(WorkerCount::Count(4)));
        // 0 loads as given; it means one worker, as `-n 0` does.
        assert_eq!(workers("workers = 0"), Some(WorkerCount::Count(0)));
        assert_eq!(workers("workers = \"auto\""), Some(WorkerCount::Auto));
        for (bad, shown) in [
            ("\"lots\"", "workers = \"lots\""),
            ("-1", "workers = -1"),
            ("2.5", "invalid type"),
            ("99999999999", "workers = 99999999999: too many"),
        ] {
            let err = load(&format!("workers = {bad}")).unwrap_err().to_string();
            assert!(err.contains(shown), "{bad}: {err}");
            assert!(!err.contains("untagged"), "{bad}: {err}");
            if bad != "99999999999" {
                assert!(
                    err.contains("a number of workers or \"auto\""),
                    "{bad}: {err}"
                );
            }
        }
    }

    /// Finding C34: `timeout`, `python` and `adapters` took any value.
    #[test]
    fn every_tool_aye_value_is_checked() {
        let dir = tempfile::tempdir().unwrap();
        let load = |body: &str| {
            write(
                dir.path(),
                "pyproject.toml",
                &format!("[tool.aye]\n{body}\n"),
            );
            AyeSettings::load(dir.path())
        };
        for (bad, shown) in [
            (
                "timeout = -5",
                "timeout = -5: expected a number of seconds, 0 or more",
            ),
            ("timeout = nan", "timeout = NaN"),
            ("timeout = inf", "timeout = inf"),
            ("timeout = \"5\"", "invalid type"),
            ("python = \"\"", "python = \"\": expected the path or name"),
            ("python = \"  \"", "python = \"  \""),
            ("python = 3", "invalid type"),
            ("adapters = []", "adapters = []: list at least one adapter"),
            (
                "adapters = [\"pytest\", \"pytest\"]",
                "\"pytest\" is listed twice",
            ),
            ("adapters = [\"\"]", "\"\" is not an adapter's name"),
            ("adapters = \"pytest\"", "invalid type"),
            ("unittest = 1", "invalid type"),
            ("doctest = \"x\"", "invalid type"),
            (
                "keep-runs = -1",
                "keep-runs = -1: expected a number of runs, 0 or more",
            ),
            ("keep-runs = 2.5", "invalid type"),
            ("keep-runs = \"10\"", "invalid type"),
            ("keep_runs = 3", "keep_runs"),
        ] {
            let err = load(bad).unwrap_err().to_string();
            assert!(err.contains(shown), "{bad}: {err}");
            assert!(err.starts_with("invalid [tool.aye] in "), "{bad}: {err}");
        }
        let config = load("timeout = 0\npython = \"python3\"\nadapters = [\"unittest\"]").unwrap();
        assert_eq!(
            config.timeout.as_ref().map(|t| t.value.seconds()),
            Some(0.0)
        );
        assert_eq!(config.timeout(), None);
        assert_eq!(load("timeout = 5").unwrap().timeout(), Some(5.0));
        assert_eq!(load("timeout = 2.5").unwrap().timeout(), Some(2.5));
        assert_eq!(load("").unwrap().keep_runs(), 10);
        assert_eq!(load("keep-runs = 0").unwrap().keep_runs(), 0);
        let config = load("keep-runs = 3").unwrap();
        assert_eq!(config.keep_runs(), 3);
        assert_eq!(
            config.entries()[0],
            (
                "keep-runs",
                "3".to_string(),
                &ValueSource::file(dir.path().join("pyproject.toml"), "[tool.aye]")
            )
        );
        let config = load("[tool.aye.unittest]\npattern = \"*_test.py\"").unwrap();
        assert!(config.adapter_table("unittest").is_some());
        assert!(config.adapter_table("doctest").is_none());
        assert!(config.adapter_table("pytest").is_none());
    }

    /// With several bad values, the one reported is the first of: an
    /// unknown key, a value of the wrong type or a bad `workers` (the first
    /// of those by the keys' names, whatever the file's order), then
    /// `output`, `python`, `timeout`, `keep-runs` and `adapters`, in that
    /// order. Pinned as 0.0.1 decides it: a usage error names one value.
    #[test]
    fn the_value_reported_among_bad_ones_is_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let first = |body: &str| {
            write(
                dir.path(),
                "pyproject.toml",
                &format!("[tool.aye]\n{body}\n"),
            );
            let err = AyeSettings::load(dir.path()).unwrap_err().to_string();
            let (_, message) = err.split_once("pyproject.toml: ").unwrap();
            message.to_string()
        };
        for (body, reported) in [
            (
                "timeout = -5\noutput = \"xml\"",
                "output = \"xml\": expected text or json",
            ),
            (
                "timeout = -5\npython = \"\"",
                "python = \"\": expected the path or name of an interpreter",
            ),
            (
                "adapters = []\nkeep-runs = -1",
                "keep-runs = -1: expected a number of runs, 0 or more",
            ),
            (
                "adapters = []\ntimeout = inf",
                "timeout = inf: expected a number of seconds, 0 or more",
            ),
            (
                "output = \"xml\"\nworkers = \"lots\"",
                "workers = \"lots\": expected a number of workers or \"auto\"",
            ),
            (
                "output = \"xml\"\ntimeout = \"5\"",
                "invalid type: string \"5\", expected f64",
            ),
            (
                "output = \"xml\"\nwrokers = 4",
                "unknown field `wrokers`, expected one of `workers`, `python`, `output`, \
                 `timeout`, `adapters`, `keep-runs`, `unittest`, `doctest`",
            ),
            (
                "workers = \"lots\"\nadapters = \"pytest\"",
                "invalid type: string \"pytest\", expected a sequence",
            ),
            (
                "adapters = \"pytest\"\nworkers = \"lots\"",
                "invalid type: string \"pytest\", expected a sequence",
            ),
            (
                "keep-runs = \"ten\"\npython = 3",
                "invalid type: string \"ten\", expected i64",
            ),
        ] {
            assert_eq!(first(body), reported, "{body}");
        }
    }

    /// What `aye config` lists of `[tool.aye]`, each value as it was set.
    #[test]
    fn every_setting_set_is_an_entry() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "pyproject.toml",
            "[tool.aye]\nkeep-runs = 3\nadapters = [\"unittest\", \"pytest\"]\ntimeout = 5.0\n\
             output = \"JSON\"\npython = \".venv/bin/python\"\nworkers = 0\n",
        );
        let settings = AyeSettings::load(dir.path()).unwrap();
        let entries: Vec<(&str, String)> = settings
            .entries()
            .into_iter()
            .map(|(name, value, source)| {
                assert_eq!(
                    source.display_row(dir.path()).to_string(),
                    "pyproject.toml [tool.aye]"
                );
                (name, value)
            })
            .collect();
        assert_eq!(
            entries,
            [
                ("workers", "0".to_string()),
                ("python", ".venv/bin/python".to_string()),
                ("output", "json".to_string()),
                ("timeout", "5".to_string()),
                ("adapters", "unittest, pytest".to_string()),
                ("keep-runs", "3".to_string()),
            ]
        );
    }

    #[test]
    fn missing_tool_aye_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(AyeSettings::load(dir.path()).unwrap(), AyeSettings::NONE);
        write(dir.path(), "pyproject.toml", "[project]\nname = \"p\"\n");
        let settings = AyeSettings::load(dir.path()).unwrap();
        assert_eq!(settings, AyeSettings::NONE);
        assert_eq!(settings.entries(), Vec::new());
    }
}
