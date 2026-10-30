//! The refinement cache's key ([`RefineKey`]), from what it holds, as
//! types: the run's inputs ([`RunKey`]) and the adapter's ([`AdapterKey`]),
//! both hashed once ([`BaseKey`]), then the file's ([`FileKey`]). Each says
//! the bytes it is hashed as, part by part: a key is the 128-bit FNV-1a hash
//! of its parts, each after its length, so the same inputs make the same key
//! in every aye of the same [`FORMAT`], and its entries keep answering.

use std::borrow::Cow;
use std::fmt;
use std::path::Path;

use aye_adapter::Setting;
use aye_events::{AdapterName, ContentDigest, Fnv128, ProjectPath};

use super::strings_in;

/// The cache's own format, part of every key: entries written under other
/// rules are never read. Changing what a key holds, or how it is hashed,
/// bumps it.
pub(super) const FORMAT: &str = "refine-2";

/// What every key holds of the run: what refinement depends on outside the
/// project's files and the adapter.
pub(super) struct RunKey<'a> {
    /// aye's version.
    pub(super) version: &'a str,
    pub(super) root: &'a Path,
    pub(super) cwd: &'a Path,
    /// The environment refinement runs in, as its fingerprint's digest.
    pub(super) environment: ContentDigest,
    /// The arguments after `--`, as adapters were given them.
    pub(super) framework_args: &'a [String],
}

impl RunKey<'_> {
    /// Its parts, in the order hashed: [`FORMAT`], aye's version, the root
    /// and the working directory as the OS encodes them, the environment's
    /// digest and the framework arguments' (each argument a part of their
    /// hash), as 16 little-endian bytes each.
    pub(super) fn parts(&self) -> [Cow<'_, [u8]>; 6] {
        let mut args = Fnv128::new();
        for arg in self.framework_args {
            args.part(arg.as_bytes());
        }
        [
            Cow::Borrowed(FORMAT.as_bytes()),
            Cow::Borrowed(self.version.as_bytes()),
            Cow::Borrowed(self.root.as_os_str().as_encoded_bytes()),
            Cow::Borrowed(self.cwd.as_os_str().as_encoded_bytes()),
            Cow::Owned(self.environment.to_le_bytes().to_vec()),
            Cow::Owned(ContentDigest::from(args).to_le_bytes().to_vec()),
        ]
    }
}

/// What every key of an adapter's entries holds of the adapter.
pub(super) struct AdapterKey<'a> {
    pub(super) name: &'a AdapterName,
    /// Its recognizer's context, with the content of every file it names:
    /// pytest's inifile holds options aye doesn't read itself.
    pub(super) context: &'a serde_json::Value,
    /// Its recognizer's settings.
    pub(super) settings: &'a [Setting],
}

impl AdapterKey<'_> {
    /// Its parts, in the order hashed: the adapter's name, its context as
    /// JSON, then for each of the context's strings (depth first) that is
    /// the absolute path of a file that can be read, the path and the
    /// file's content, then each setting's name and value.
    pub(super) fn parts(&self) -> Vec<Cow<'_, [u8]>> {
        let mut parts = vec![
            Cow::Borrowed(self.name.as_str().as_bytes()),
            Cow::Owned(self.context.to_string().into_bytes()),
        ];
        let mut strings = Vec::new();
        strings_in(self.context, &mut strings);
        for text in strings {
            let path = Path::new(text);
            if path.is_absolute()
                && let Ok(bytes) = std::fs::read(path)
            {
                parts.push(Cow::Borrowed(text.as_bytes()));
                parts.push(Cow::Owned(bytes));
            }
        }
        for setting in self.settings {
            parts.push(Cow::Borrowed(setting.name.as_bytes()));
            parts.push(Cow::Borrowed(setting.value.as_bytes()));
        }
        parts
    }
}

/// Everything in a key but the file: the run's parts, then the adapter's,
/// hashed once for all of the adapter's files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BaseKey(u128);

impl BaseKey {
    pub(super) fn new(run: &RunKey<'_>, adapter: &AdapterKey<'_>) -> Self {
        Self::of(run.parts().into_iter().chain(adapter.parts()))
    }

    /// The hash of `parts`, in order.
    pub(super) fn of<P: AsRef<[u8]>>(parts: impl IntoIterator<Item = P>) -> Self {
        Self(hash(parts))
    }
}

/// One file's key: the base, the file, and what importing it runs first.
pub(super) struct FileKey<'a> {
    pub(super) base: BaseKey,
    pub(super) path: &'a ProjectPath,
    pub(super) content: ContentDigest,
    /// Its support files, then its packages' `__init__.py` files, each
    /// outermost first.
    pub(super) runs_first: &'a [(ProjectPath, ContentDigest)],
}

impl FileKey<'_> {
    /// Its parts, in the order hashed: the base, as 16 little-endian bytes,
    /// the file's path and its content's digest, then each of
    /// [`FileKey::runs_first`], its path and its digest; the digests as 16
    /// little-endian bytes.
    pub(super) fn parts(&self) -> Vec<Cow<'_, [u8]>> {
        let mut parts = vec![
            Cow::Owned(self.base.0.to_le_bytes().to_vec()),
            Cow::Borrowed(self.path.as_str().as_bytes()),
            Cow::Owned(self.content.to_le_bytes().to_vec()),
        ];
        for (path, digest) in self.runs_first {
            parts.push(Cow::Borrowed(path.as_str().as_bytes()));
            parts.push(Cow::Owned(digest.to_le_bytes().to_vec()));
        }
        parts
    }

    pub(super) fn key(&self) -> RefineKey {
        RefineKey(hash(self.parts()))
    }
}

/// An entry's key: 128 bits, whose 32 lower-case hex digits name its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct RefineKey(u128);

impl fmt::Display for RefineKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

/// The FNV-1a hash of `parts`, in order, each after its length.
fn hash<P: AsRef<[u8]>>(parts: impl IntoIterator<Item = P>) -> u128 {
    let mut hash = Fnv128::new();
    for part in parts {
        hash.part(part.as_ref());
    }
    hash.finish()
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::path::Path;

    use aye_adapter::Setting;
    use aye_config::ValueSource;
    use aye_events::{AdapterName, Case, Certainty, ContentDigest, Fnv128, ProjectPath};
    use aye_protocol::Refined;

    use super::{AdapterKey, BaseKey, FORMAT, FileKey, RunKey};
    use crate::refine::cache::RefineCache;

    fn setting(name: &'static str, value: &str) -> Setting {
        Setting {
            name: name.into(),
            value: value.into(),
            source: ValueSource::Default,
        }
    }

    fn owned(parts: Vec<Cow<'_, [u8]>>) -> Vec<Vec<u8>> {
        parts.into_iter().map(Cow::into_owned).collect()
    }

    /// A cache entry's key, from the run's inputs, the adapter's, the file
    /// and its support files, pinned: entries written by another aye of
    /// the same `FORMAT` must keep answering. Changing what the key holds
    /// or how it is hashed bumps `FORMAT`.
    #[test]
    fn keys_are_pinned() {
        let settings = [setting("python_files", "test_*.py")];
        let name = AdapterName::from("pytest");
        let context = serde_json::json!({"native": {"rootdir": "relative", "args": ["-x"]}});
        let adapter = AdapterKey {
            name: &name,
            context: &context,
            settings: &settings,
        };
        let run: [&[u8]; 5] = [
            FORMAT.as_bytes(),
            b"0.0.1",
            b"/root",
            b"/root",
            &7u128.to_le_bytes(),
        ];
        let base = BaseKey::of(run.into_iter().map(Cow::Borrowed).chain(adapter.parts()));
        let cache = RefineCache::new(Path::new("/cache"), &name, base);
        let file = ContentDigest::of(b"def test_a(): pass\n");
        let support = [(
            ProjectPath::as_written("conftest.py"),
            ContentDigest::of(b"import pytest\n"),
        )];
        let key = cache.key(&ProjectPath::as_written("tests/test_a.py"), file, &support);
        assert_eq!(key.to_string(), "2b8d189545df6dd813e51e0ff1f3a71b");
        assert_eq!(
            cache.path(key),
            Path::new("/cache/refine/pytest/2b8d189545df6dd813e51e0ff1f3a71b.json")
        );
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
            setting("python_files", "test_*.py"),
            setting("python_classes", "Test"),
        ];
        let context = serde_json::json!({"rootdir": "/nowhere", "args": ["-x"]});
        let args = hashed(&[b"-x", b"--maxfail=2"]);
        let environment = ContentDigest::of(b"an environment");
        let framework_args = ["-x".to_string(), "--maxfail=2".to_string()];
        let run = RunKey {
            version: "0.0.1",
            root: Path::new("/root"),
            cwd: Path::new("/root/tests"),
            environment,
            framework_args: &framework_args,
        };
        let name = AdapterName::from("pytest");
        let adapter = AdapterKey {
            name: &name,
            context: &context,
            settings: &settings,
        };
        let parts: [&[u8]; 12] = [
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
        ];
        assert_eq!(owned(run.parts().into()), parts[..6]);
        assert_eq!(owned(adapter.parts()), parts[6..]);
        let base = BaseKey::new(&run, &adapter);
        assert_eq!(base, BaseKey(hashed(&parts)));
        let file = ContentDigest::of(b"def test_a(): pass\n");
        let conftest = ContentDigest::of(b"import pytest\n");
        let init = ContentDigest::of(b"");
        let runs_first = [
            (ProjectPath::as_written("conftest.py"), conftest),
            (ProjectPath::as_written("tests/pkg/__init__.py"), init),
        ];
        let path = ProjectPath::as_written("tests/pkg/test_a.py");
        let file_key = FileKey {
            base,
            path: &path,
            content: file,
            runs_first: &runs_first,
        };
        let parts: [&[u8]; 7] = [
            &base.0.to_le_bytes(),
            b"tests/pkg/test_a.py",
            &file.to_le_bytes(),
            b"conftest.py",
            &conftest.to_le_bytes(),
            b"tests/pkg/__init__.py",
            &init.to_le_bytes(),
        ];
        assert_eq!(owned(file_key.parts()), parts);
        let key = file_key.key();
        assert_eq!(key.to_string(), format!("{:032x}", hashed(&parts)));
        assert_eq!(key.to_string(), "2cad17ddd4c487ea6ea24d097c6f0e80");

        // A file the context names, by its path and then its content.
        let dir = tempfile::tempdir().unwrap();
        let inifile = dir.path().join("pytest.ini");
        std::fs::write(&inifile, "[pytest]\naddopts = -x\n").unwrap();
        let inifile = inifile.to_str().unwrap();
        let context = serde_json::json!({"inifile": inifile, "rootdir": "/nowhere"});
        let adapter = AdapterKey {
            name: &name,
            context: &context,
            settings: &[],
        };
        let base = BaseKey::of(
            [Cow::Borrowed(FORMAT.as_bytes())]
                .into_iter()
                .chain(adapter.parts()),
        );
        let cache = RefineCache::new(Path::new("/cache"), &name, base);
        let base = hashed(&[
            b"refine-2",
            b"pytest",
            context.to_string().as_bytes(),
            inifile.as_bytes(),
            b"[pytest]\naddopts = -x\n",
        ]);
        assert_eq!(
            cache
                .key(&ProjectPath::as_written("t.py"), file, &[])
                .to_string(),
            format!(
                "{:032x}",
                hashed(&[&base.to_le_bytes(), b"t.py", &file.to_le_bytes()])
            )
        );

        // The entry: `<key>.json` under `refine/<adapter>`, holding the
        // result as its JSON, and the state directory marked as a cache.
        let state = dir.path().join(".aye");
        let top = state.join("cache");
        let cache = RefineCache::new(&top, &name, BaseKey(0));
        let mut case = Case::new("tests/test_a.py::test_a[1]", "pytest");
        case.certainty = Certainty::Refined;
        let refined = Refined {
            cases: vec![case],
            diagnostics: Vec::new(),
            digests: None,
        };
        cache.write(key, &refined);
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
        assert_eq!(cache.read(key), Some(refined));
    }
}
