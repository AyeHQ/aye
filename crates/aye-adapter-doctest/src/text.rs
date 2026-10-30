//! A text file's doctest: one case, when the file holds examples.

use aye_adapter::{Recognition, SourceFile};
use aye_events::{AffinityKey, Span};

use crate::{DoctestRecognizer, parser};

impl DoctestRecognizer {
    pub(crate) fn recognize_text(&self, file: &SourceFile<'_>) -> Recognition {
        let mut recognition = Recognition::default();
        let encoding = self
            .config
            .encoding
            .value
            .to_lowercase()
            .replace(['-', '_'], "");
        if !matches!(encoding.as_str(), "utf8" | "u8" | "utf" | "cp65001") {
            recognition.refine =
                Some(format!("text files are read as {}", self.config.encoding.value).into());
        }
        let doc = parser::text_file(&parser::universal_newlines(file.source));
        if let Some(why) = doc.unsure {
            recognition.refine = Some(why.into());
        }
        if doc.examples {
            if let Some(why) = self.skipping_conftest(file.abs) {
                recognition.refine = Some(why.into());
            }
            let name = file.rel.file_name();
            let span = Span {
                path: file.rel.to_project_path(),
                start: 0,
                end: 0,
                line: Some(1),
            };
            let case = self.case(file, name, span, vec![AffinityKey::file(file.rel)]);
            recognition.cases.push(case);
        }
        recognition
    }
}
