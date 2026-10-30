//! The refinement cache on disk: one entry per file and adapter, read
//! when it holds a clean result and written aside, then renamed.

use std::path::{Path, PathBuf};

use aye_adapter::Setting;
use aye_events::{AdapterName, ContentDigest, Fnv128};
use aye_protocol::Refined;

use super::strings_in;

/// Refinement results on disk, one JSON file per file and adapter:
/// `<cache dir>/refine/<adapter>/<key>.json`.
pub(super) struct Cache {
    /// The cache directory, created through `aye_fs::prepare_dir` so
    /// that the state directory holding it is marked as a cache.
    top: PathBuf,
    dir: PathBuf,
    /// Everything in the key that doesn't depend on the file.
    base: u128,
}

impl Cache {
    /// `parts` are the run's inputs (format, version, root, cwd,
    /// environment, the project's framework arguments), hashed before the
    /// adapter's own.
    pub(super) fn new(
        cache_dir: &Path,
        parts: &[&[u8]],
        adapter: &AdapterName,
        context: &serde_json::Value,
        settings: &[Setting],
    ) -> Self {
        let mut hash = Fnv128::new();
        for part in parts {
            hash.part(part);
        }
        hash.part(adapter.as_str().as_bytes());
        hash.part(context.to_string().as_bytes());
        // The content of the files the context names: pytest's inifile holds
        // options aye doesn't read itself.
        let mut strings = Vec::new();
        strings_in(context, &mut strings);
        for text in strings {
            let path = Path::new(text);
            if path.is_absolute()
                && let Ok(bytes) = std::fs::read(path)
            {
                hash.part(text.as_bytes());
                hash.part(&bytes);
            }
        }
        for setting in settings {
            hash.part(setting.name.as_bytes());
            hash.part(setting.value.as_bytes());
        }
        Self {
            top: cache_dir.to_path_buf(),
            dir: cache_dir.join(aye_fs::REFINE_CACHE).join(adapter.as_str()),
            base: hash.finish(),
        }
    }

    pub(super) fn key<'a>(
        &self,
        rel: &str,
        digest: ContentDigest,
        support: impl Iterator<Item = (&'a str, ContentDigest)>,
    ) -> String {
        let mut hash = Fnv128::new();
        hash.part(&self.base.to_le_bytes());
        hash.part(rel.as_bytes());
        hash.part(&digest.to_le_bytes());
        for (rel, digest) in support {
            hash.part(rel.as_bytes());
            hash.part(&digest.to_le_bytes());
        }
        format!("{:032x}", hash.finish())
    }

    pub(super) fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    /// An entry, when it holds a clean result (see [`Cache::cacheable`]).
    pub(super) fn read(&self, key: &str) -> Option<Refined> {
        let bytes = std::fs::read(self.path(key)).ok()?;
        serde_json::from_slice(&bytes).ok().filter(Self::cacheable)
    }

    /// Only a clean result is cached: one with cases and no diagnostic. An
    /// import error or a module-level skip may be the environment's, and
    /// fixed without touching the file; a file with no tests at all costs
    /// little to refine again, and a stale one would hide tests.
    pub(super) fn cacheable(refined: &Refined) -> bool {
        !refined.cases.is_empty() && refined.diagnostics.is_empty()
    }

    /// Best effort: a cache that can't be written only costs time. Written
    /// aside and renamed, so a concurrent run never reads half an entry.
    pub(super) fn write(&self, key: &str, refined: &Refined) {
        if !Self::cacheable(refined) {
            return;
        }
        let Ok(bytes) = serde_json::to_vec(refined) else {
            return;
        };
        let aside = self.dir.join(format!("{key}.{}.tmp", std::process::id()));
        let written = aye_fs::prepare_dir(&self.top)
            .and_then(|()| std::fs::create_dir_all(&self.dir))
            .and_then(|()| std::fs::write(&aside, bytes))
            .and_then(|()| std::fs::rename(&aside, self.path(key)));
        if written.is_err() {
            let _ = std::fs::remove_file(&aside);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use aye_adapter::Setting;
    use aye_config::ValueSource;
    use aye_events::{AdapterName, ContentDigest, Fnv128};
    use aye_protocol::Refined;

    use super::Cache;
    use crate::refine::FORMAT;

    /// A cache entry's key, from the run's inputs, the adapter's, the file
    /// and its support files, pinned: entries written by another aye of
    /// the same `FORMAT` must keep answering. Changing what the key holds
    /// or how it is hashed bumps `FORMAT`.
    #[test]
    fn keys_are_pinned() {
        let settings = [Setting {
            name: "python_files".into(),
            value: "test_*.py".into(),
            source: ValueSource::Default,
        }];
        let cache = Cache::new(
            Path::new("/cache"),
            &[
                FORMAT.as_bytes(),
                b"0.0.1",
                b"/root",
                b"/root",
                &7u128.to_le_bytes(),
            ],
            &AdapterName::from("pytest"),
            &serde_json::json!({"native": {"rootdir": "relative", "args": ["-x"]}}),
            &settings,
        );
        let file = ContentDigest::of(b"def test_a(): pass\n");
        let support = [("conftest.py", ContentDigest::of(b"import pytest\n"))];
        assert_eq!(
            cache.key("tests/test_a.py", file, support.into_iter()),
            "2b8d189545df6dd813e51e0ff1f3a71b"
        );
        assert_eq!(cache.path("k"), Path::new("/cache/refine/pytest/k.json"));
    }

    /// The hash of `parts`, each after its length, in order: how every key
    /// is hashed.
    fn hashed(parts: &[&[u8]]) -> u128 {
        let mut hash = Fnv128::new();
        for part in parts {
            hash.part(part);
        }
        hash.finish()
    }

    /// The bytes a key is hashed from, part by part, as a run gives them
    /// (with the framework arguments' digest, which `keys_are_pinned`
    /// leaves out), and the entry its key names: its file's name and the
    /// bytes in it.
    #[test]
    fn a_keys_parts_and_its_entry_are_pinned() {
        let settings = [
            Setting {
                name: "python_files".into(),
                value: "test_*.py".into(),
                source: ValueSource::Default,
            },
            Setting {
                name: "python_classes".into(),
                value: "Test".into(),
                source: ValueSource::Default,
            },
        ];
        let context = serde_json::json!({"rootdir": "/nowhere", "args": ["-x"]});
        let args = hashed(&[b"-x", b"--maxfail=2"]);
        let environment = ContentDigest::of(b"an environment");
        let cache = Cache::new(
            Path::new("/cache"),
            &[
                FORMAT.as_bytes(),
                b"0.0.1",
                b"/root",
                b"/root/tests",
                &environment.to_le_bytes(),
                &args.to_le_bytes(),
            ],
            &AdapterName::from("pytest"),
            &context,
            &settings,
        );
        let base = hashed(&[
            b"refine-2",
            b"0.0.1",
            b"/root",
            b"/root/tests",
            &environment.to_le_bytes(),
            &args.to_le_bytes(),
            b"pytest",
            br#"{"args":["-x"],"rootdir":"/nowhere"}"#,
            b"python_files",
            b"test_*.py",
            b"python_classes",
            b"Test",
        ]);
        let file = ContentDigest::of(b"def test_a(): pass\n");
        let conftest = ContentDigest::of(b"import pytest\n");
        let init = ContentDigest::of(b"");
        let key = cache.key(
            "tests/pkg/test_a.py",
            file,
            [("conftest.py", conftest), ("tests/pkg/__init__.py", init)].into_iter(),
        );
        let expected = hashed(&[
            &base.to_le_bytes(),
            b"tests/pkg/test_a.py",
            &file.to_le_bytes(),
            b"conftest.py",
            &conftest.to_le_bytes(),
            b"tests/pkg/__init__.py",
            &init.to_le_bytes(),
        ]);
        assert_eq!(key, format!("{expected:032x}"));
        assert_eq!(key, "2cad17ddd4c487ea6ea24d097c6f0e80");

        // A file the context names, by its path and then its content.
        let dir = tempfile::tempdir().unwrap();
        let inifile = dir.path().join("pytest.ini");
        std::fs::write(&inifile, "[pytest]\naddopts = -x\n").unwrap();
        let inifile = inifile.to_str().unwrap();
        let context = serde_json::json!({"inifile": inifile, "rootdir": "/nowhere"});
        let cache = Cache::new(
            Path::new("/cache"),
            &[FORMAT.as_bytes()],
            &AdapterName::from("pytest"),
            &context,
            &[],
        );
        let base = hashed(&[
            b"refine-2",
            b"pytest",
            context.to_string().as_bytes(),
            inifile.as_bytes(),
            b"[pytest]\naddopts = -x\n",
        ]);
        assert_eq!(
            cache.key("t.py", file, std::iter::empty()),
            format!(
                "{:032x}",
                hashed(&[&base.to_le_bytes(), b"t.py", &file.to_le_bytes()])
            )
        );

        // The entry: `<key>.json` under `refine/<adapter>`, holding the
        // result as its JSON, and the state directory marked as a cache.
        let state = dir.path().join(".aye");
        let top = state.join("cache");
        let cache = Cache {
            top: top.clone(),
            dir: top.join("refine").join("pytest"),
            base: 0,
        };
        let mut case = aye_events::Case::new("tests/test_a.py::test_a[1]", "pytest");
        case.certainty = aye_events::Certainty::Refined;
        let refined = Refined {
            cases: vec![case],
            diagnostics: Vec::new(),
            digests: None,
        };
        cache.write("2cad17ddd4c487ea6ea24d097c6f0e80", &refined);
        let names = |dir: &Path| {
            let mut names: Vec<String> = std::fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        };
        assert_eq!(
            names(&top.join("refine/pytest")),
            ["2cad17ddd4c487ea6ea24d097c6f0e80.json"]
        );
        assert_eq!(names(&state), [".gitignore", "CACHEDIR.TAG", "cache"]);
        assert_eq!(
            std::fs::read_to_string(
                top.join("refine/pytest/2cad17ddd4c487ea6ea24d097c6f0e80.json")
            )
            .unwrap(),
            r#"{"cases":[{"id":"tests/test_a.py::test_a[1]","adapter":"pytest","certainty":"refined"}],"diagnostics":[]}"#
        );
        assert_eq!(
            cache.read("2cad17ddd4c487ea6ea24d097c6f0e80"),
            Some(refined)
        );
    }
}
