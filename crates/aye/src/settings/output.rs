//! What a run's output is ([`OutputSettings`]), and its format, which `aye
//! show` takes as a run does ([`format`]).
//!
//! Decided (implementation, October 2026):
//! - **Two output formats, one scale of detail** (October 2026): `--output
//!   text` (the default) or `json` (JSON lines), else `[tool.aye] output`.
//!   The verbosity, the number of `-v` less the number of `-q`, clamped to
//!   -1..3 (`aye-render`'s `Level`), shapes either (`Text` and
//!   `JsonLines`): `-q` the summary alone, `-v` what used to be the agent
//!   output in JSON, `-vvv` everything (the raw event stream in JSON). A
//!   batch's policy still carries the count unclamped
//!   (`Policy::verbosity`).
//!   `human` and `agent`, the formats' names until then (never published),
//!   are usage errors that say what to use instead (`text`; `json -v`), on
//!   the command line and in `[tool.aye]`. `aye config` shows the format at
//!   its verbosity (`json -v`) and where the format came from.

use aye_config::{AyeSettings, OutputFormat, Sourced, ValueSource};
use aye_render::ColorChoice;

use crate::cli::{Color, Verbosity};

/// The run's output: the format, and where it came from; how much it shows
/// (`-v`, `-q`); and when to use colour.
#[derive(Debug, Clone)]
pub(crate) struct OutputSettings {
    pub(crate) format: Sourced<OutputFormat>,
    pub(crate) verbosity: Verbosity,
    pub(crate) color: ColorChoice,
}

impl OutputSettings {
    /// `--output` (`flag`), else `[tool.aye] output`, else text, at
    /// `verbosity`, in `color`.
    pub(crate) fn resolve(
        flag: Option<OutputFormat>,
        tool_aye: &AyeSettings,
        verbosity: Verbosity,
        color: Color,
    ) -> Self {
        Self {
            format: format(flag, tool_aye),
            verbosity,
            color: color.into(),
        }
    }

    /// The format at its verbosity, as the command line would ask for it
    /// (`json -v`), for `aye config`.
    pub(crate) fn shown(&self) -> String {
        format!("{}{}", self.format.value.as_str(), self.verbosity)
    }
}

/// The output format, and where it came from: `--output`, else
/// `[tool.aye] output`, else text.
pub(crate) fn format(flag: Option<OutputFormat>, tool_aye: &AyeSettings) -> Sourced<OutputFormat> {
    if let Some(output) = flag {
        return Sourced::new(output, ValueSource::Cli("--output"));
    }
    tool_aye
        .output
        .clone()
        .unwrap_or(Sourced::default_value(OutputFormat::Text))
}

impl From<Color> for ColorChoice {
    fn from(color: Color) -> Self {
        match color {
            Color::Auto => Self::Auto,
            Color::Always => Self::Always,
            Color::Never => Self::Never,
        }
    }
}
