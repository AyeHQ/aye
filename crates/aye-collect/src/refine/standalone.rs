//! Whether a refined file's tests can depend on anything the refinement
//! cache's key leaves out.
//!
//! The key holds the file and its support files, the interpreter and its
//! installed packages (by name: the entries of its `site-packages`, not
//! their content), the environment, the working directory, the recognizer's
//! context and settings and the framework's arguments. What the import of a
//! file reads beyond that (a helper module of the project, an installed
//! module edited in place or installed in editable mode outside the root, a
//! data file, the clock) is unknown without the worker reporting it, so a
//! file is cached only when it and each of its support files are
//! *standalone*: it imports nothing but pure standard-library modules and
//! the adapter's framework, never touches the filesystem, other processes,
//! the clock or randomness, never runs code by name (`__import__`,
//! `import_module`, `importorskip`), and names no module of the project and
//! no installed module in a string (a plugin list, a module to import
//! later). The check is lexical and walks every function body, including
//! those that never run at import, so it errs towards refining again.
//!
//! The framework's arguments and the recognizer's context are checked the
//! same way ([`loads_by_name`]): a plugin option (`-p NAME`) that loads any
//! module but the framework or the pure standard library, or any value
//! naming a module of the project or an installed one, turns the cache off.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aye_adapter::ast::visitor::{self, Visitor};
use aye_adapter::ast::{self, Expr, Stmt};
use aye_provision::InstalledModules;

/// Standard-library modules whose import can't reach outside the key: no
/// filesystem walks, processes, network, clock or randomness of their own.
/// Some reach the filesystem through a few functions, which
/// [`IMPURE_ATTRIBUTES`] catches.
const PURE_MODULES: &[&str] = &[
    "__future__",
    "abc",
    "argparse",
    "array",
    "ast",
    "asyncio",
    "atexit",
    "base64",
    "binascii",
    "bisect",
    "builtins",
    "calendar",
    "cmath",
    "codecs",
    "collections",
    "colorsys",
    "contextlib",
    "contextvars",
    "copy",
    "copyreg",
    "csv",
    "dataclasses",
    "datetime",
    "decimal",
    "difflib",
    "dis",
    "email",
    "enum",
    "errno",
    "faulthandler",
    "fnmatch",
    "fractions",
    "functools",
    "gc",
    "genericpath",
    "getopt",
    "graphlib",
    "hashlib",
    "heapq",
    "hmac",
    "html",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "keyword",
    "locale",
    "logging",
    "marshal",
    "math",
    "ntpath",
    "numbers",
    "operator",
    "optparse",
    "os",
    "pathlib",
    "pickle",
    "platform",
    "posixpath",
    "pprint",
    "queue",
    "quopri",
    "re",
    "reprlib",
    "shlex",
    "signal",
    "stat",
    "statistics",
    "string",
    "stringprep",
    "struct",
    "sys",
    "textwrap",
    "threading",
    "token",
    "tokenize",
    "tomllib",
    "traceback",
    "types",
    "typing",
    "unicodedata",
    "unittest",
    "warnings",
    "weakref",
    "zlib",
    "zoneinfo",
];

/// Attributes that read the filesystem, start or inspect processes, read the
/// clock or randomness, or import by name. Used as an attribute
/// (`path.read_text()`, `os.listdir`) or imported (`from os import
/// listdir`), each makes a file not standalone. Names the allowed modules
/// only reach through a module that isn't allowed (`time.time`) are left
/// out, and so are names too common elsewhere to tell apart (`re.compile`).
const IMPURE_ATTRIBUTES: &[&str] = &[
    // The filesystem.
    "open",
    "open_code",
    "fdopen",
    "read_text",
    "read_bytes",
    "glob",
    "iglob",
    "rglob",
    "iterdir",
    "listdir",
    "scandir",
    "walk",
    "fwalk",
    "exists",
    "lexists",
    "is_file",
    "is_dir",
    "is_symlink",
    "is_mount",
    "is_junction",
    "isfile",
    "isdir",
    "islink",
    "ismount",
    "isjunction",
    "stat",
    "lstat",
    "getsize",
    "getmtime",
    "getatime",
    "getctime",
    "samefile",
    "readlink",
    "access",
    "fileConfig",
    "getsource",
    "getsourcelines",
    "getsourcefile",
    "findsource",
    "FileIO",
    // Processes.
    "system",
    "popen",
    "startfile",
    "spawnl",
    "spawnle",
    "spawnlp",
    "spawnlpe",
    "spawnv",
    "spawnve",
    "spawnvp",
    "spawnvpe",
    "execl",
    "execle",
    "execlp",
    "execlpe",
    "execv",
    "execve",
    "execvp",
    "execvpe",
    "fork",
    "forkpty",
    "posix_spawn",
    "posix_spawnp",
    "getpid",
    "getppid",
    "getloadavg",
    "create_subprocess_exec",
    "create_subprocess_shell",
    // The network.
    "open_connection",
    "open_unix_connection",
    "getaddrinfo",
    // The clock and randomness.
    "now",
    "today",
    "utcnow",
    "urandom",
    "getrandom",
    // Code by name.
    "__import__",
    "import_module",
    "importorskip",
    "exec_module",
    "load_module",
    "find_spec",
];

/// Functions that import a module named by a string. A string literal
/// spelling one of them (`vars(builtins)["__import__"]`) makes a file not
/// standalone too.
const BY_NAME: &[&str] = &["__import__", "import_module", "importorskip"];

/// Builtins that read files or the terminal or run code by name, and the
/// builtins' namespace itself. Used as a bare name, called or not, each
/// makes a file not standalone.
const IMPURE_BUILTINS: &[&str] = &[
    "open",
    "exec",
    "eval",
    "compile",
    "__import__",
    "input",
    "breakpoint",
    "execfile",
    "__builtins__",
];

/// Builtins that differ from one process to the next, when called.
const VOLATILE_CALLS: &[&str] = &["id", "hash"];

/// Where a file's imports and module names are looked up.
pub(crate) struct Scope<'a> {
    pub root: &'a Path,
    pub cwd: &'a Path,
    /// The adapter's name: its framework's top-level module (`pytest`), also
    /// allowed with a leading underscore (`_pytest`).
    pub framework: &'a str,
    /// Whether `dir/name.py` or `dir/name/` exists, by path; shared across
    /// files.
    pub seen: &'a Mutex<HashMap<PathBuf, bool>>,
    /// The top-level modules installed outside the standard library
    /// (the environment's [`InstalledModules`]).
    pub installed: &'a InstalledModules,
}

impl Scope<'_> {
    /// The directories a module of the project may be imported from, for a
    /// file in `dir`: `dir` and each directory above it up to the root (any
    /// of them may be put on `sys.path`, as pytest's rootdir-based import
    /// modes do), the working directory and `<root>/src`.
    fn import_dirs(&self, dir: &Path) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = dir
            .ancestors()
            .take_while(|d| d.starts_with(self.root))
            .map(Path::to_path_buf)
            .collect();
        for extra in [self.cwd.to_path_buf(), self.root.join("src")] {
            if !dirs.contains(&extra) {
                dirs.push(extra);
            }
        }
        dirs
    }

    /// Whether the top-level module `name` may be a file of the project,
    /// imported from one of `dirs`.
    fn in_project(&self, dirs: &[PathBuf], name: &str) -> bool {
        let mut seen = self
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        dirs.iter().any(|dir| {
            [dir.join(format!("{name}.py")), dir.join(name)]
                .into_iter()
                .any(|path| {
                    *seen.entry(path).or_insert_with_key(|path| {
                        path.is_dir() || path.extension().is_some() && path.is_file()
                    })
                })
        })
    }

    fn allowed_module(&self, name: &str) -> bool {
        PURE_MODULES.contains(&name)
            || name == self.framework
            || name.strip_prefix('_') == Some(self.framework)
    }

    /// Whether the top-level module `name` may be an installed one whose
    /// content the key doesn't hold: anything installed but the framework
    /// and the pure standard library.
    fn installed_module(&self, name: &str) -> bool {
        self.installed.contains(name) && !self.allowed_module(name)
    }

    /// Why the module `text` names, a dotted name, may be outside the key:
    /// a module of the project, or an installed one.
    fn names_module(&self, dirs: &[PathBuf], text: &str) -> Option<String> {
        let head = dotted_head(text)?;
        if self.in_project(dirs, head) {
            Some(format!("{text:?} may name a project module"))
        } else if self.installed_module(head) {
            Some(format!("{text:?} may name an installed module"))
        } else {
            None
        }
    }
}

/// Why the refined tests of the file at `abs` (or of a file it supports) may
/// depend on something outside the cache's key, or `None` when they can't.
pub(crate) fn dependence(module: &ast::ModModule, abs: &Path, scope: &Scope<'_>) -> Option<String> {
    let dir = abs.parent()?;
    if !dir.starts_with(scope.root) {
        return Some("it is outside the project".to_string());
    }
    let mut check = Check {
        scope,
        dirs: scope.import_dirs(dir),
        reason: None,
    };
    for stmt in &module.body {
        check.visit_stmt(stmt);
        if check.reason.is_some() {
            break;
        }
    }
    check.reason
}

/// Why the framework's arguments or the recognizer's context, as `texts`
/// (each split at whitespace, in order), may load code the key can't hold:
/// a plugin option, `-p NAME` or `-pNAME`, naming any module but the
/// framework or a pure standard-library one (`-p no:NAME` loads nothing; a
/// value that isn't a dotted name, such as a file pattern, is no module),
/// or any value, also after `=` or `,`, naming a module of the project or
/// an installed one (see [`Scope::installed`]). `None` when they can't.
pub(crate) fn loads_by_name<'t>(
    texts: impl IntoIterator<Item = &'t str>,
    scope: &Scope<'_>,
) -> Option<String> {
    let dirs = scope.import_dirs(scope.root);
    let mut plugin_next = false;
    for token in texts.into_iter().flat_map(str::split_whitespace) {
        let plugin = if plugin_next {
            Some(token)
        } else {
            token.strip_prefix("-p").filter(|rest| !rest.is_empty())
        };
        plugin_next = token == "-p";
        if let Some(plugin) = plugin.map(|p| p.trim_start_matches('='))
            && let Some(head) = dotted_head(plugin)
            && (!scope.allowed_module(head) || scope.in_project(&dirs, head))
        {
            return Some(format!("`-p {plugin}` loads a plugin by name"));
        }
        let values = token.strip_prefix("-p").unwrap_or(token);
        if let Some(why) = values
            .split(['=', ','])
            .find_map(|value| scope.names_module(&dirs, value))
        {
            return Some(format!("the framework's arguments: {why}"));
        }
    }
    None
}

/// The first part of a dotted name (`a` of `a.b.c`), when `text` is one.
fn dotted_head(text: &str) -> Option<&str> {
    let is_identifier = |part: &str| {
        let mut chars = part.chars();
        chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
            && chars.all(|c| c == '_' || c.is_alphanumeric())
    };
    (text.len() <= 200 && text.split('.').all(is_identifier)).then(|| {
        text.split('.')
            .next()
            .expect("split yields at least one part")
    })
}

struct Check<'s, 'a> {
    scope: &'s Scope<'a>,
    dirs: Vec<PathBuf>,
    reason: Option<String>,
}

impl Check<'_, '_> {
    fn fail(&mut self, reason: String) {
        if self.reason.is_none() {
            self.reason = Some(reason);
        }
    }

    fn import(&mut self, module: &str) {
        let head = module.split('.').next().unwrap_or(module);
        if !self.scope.allowed_module(head) {
            self.fail(format!("it imports `{module}`"));
        } else if self.scope.in_project(&self.dirs, head) {
            self.fail(format!("its import of `{module}` may be a project file"));
        }
    }
}

impl<'a> Visitor<'a> for Check<'_, '_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if self.reason.is_some() {
            return;
        }
        match stmt {
            Stmt::Import(import) => {
                for alias in &import.names {
                    self.import(alias.name.as_str());
                }
            }
            Stmt::ImportFrom(import) => {
                if import.level > 0 {
                    self.fail("it has a relative import".to_string());
                    return;
                }
                let module = import.module.as_ref().map_or("", |m| m.as_str());
                self.import(module);
                for alias in &import.names {
                    let name = alias.name.as_str();
                    if name == "*"
                        || IMPURE_ATTRIBUTES.contains(&name)
                        || IMPURE_BUILTINS.contains(&name)
                    {
                        self.fail(format!("it imports `{name}` from `{module}`"));
                    }
                }
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if self.reason.is_some() {
            return;
        }
        match expr {
            Expr::Name(name) if IMPURE_BUILTINS.contains(&name.id.as_str()) => {
                self.fail(format!("it uses `{}`", name.id));
            }
            Expr::Attribute(attribute) if IMPURE_ATTRIBUTES.contains(&attribute.attr.as_str()) => {
                self.fail(format!("it uses `.{}`", attribute.attr));
            }
            Expr::Call(call) => {
                if let Expr::Name(name) = &*call.func {
                    let callee = name.id.as_str();
                    if VOLATILE_CALLS.contains(&callee) {
                        self.fail(format!("it calls `{callee}`"));
                    } else if callee == "getattr" {
                        let literal = call.arguments.args.get(1).and_then(|arg| match arg {
                            Expr::StringLiteral(s) => Some(s.value.to_str()),
                            _ => None,
                        });
                        if literal.is_none_or(|attr| IMPURE_ATTRIBUTES.contains(&attr)) {
                            self.fail("it calls `getattr` with a name it computes".to_string());
                        }
                    }
                }
                visitor::walk_expr(self, expr);
            }
            Expr::StringLiteral(literal) => {
                let text = literal.value.to_str();
                if BY_NAME.contains(&text) {
                    self.fail(format!("its string {text:?} names a way to import by name"));
                } else if let Some(why) = self.scope.names_module(&self.dirs, text) {
                    self.fail(format!("its string {why}"));
                }
            }
            _ => visitor::walk_expr(self, expr),
        }
    }
}
