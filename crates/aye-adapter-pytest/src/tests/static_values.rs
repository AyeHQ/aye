//! aye-python-semantic's namespaces, escape analysis and static values,
//! as recognition reads them: which parametrized ids stay exact.
//!
//! Expected ids are copied from `pytest --collect-only -q` on pytest
//! 9.1.1 (Python 3.13); `static_values_match_pytest` re-checks every
//! exact reading against a real pytest when one is available.

use std::path::PathBuf;
use std::process::Command;

use aye_adapter::{LineIndex, Project, Recognition, Recognizer, Requested, SourceFile};
use aye_events::Certainty;

fn recognize(source: &str) -> Recognition {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pytest.ini"), "[pytest]\n").unwrap();
    let project = Project::new(dir.path(), dir.path());
    let recognizer = crate::PytestRecognizer::new(&project).unwrap();
    let parsed = ruff_python_parser::parse_module(source).unwrap();
    let lines = LineIndex::from_source_text(source);
    let abs = dir.path().join("test_x.py");
    let file = SourceFile {
        rel: aye_events::ProjectPathRef::as_written("test_x.py"),
        abs: &abs,
        source,
        module: parsed.syntax(),
        lines: &lines,
    };
    recognizer.recognize(&file, Requested::Found, &[])
}

/// The ids of a file read statically, or `None` when it is refined.
fn exact(source: &str) -> Option<Vec<String>> {
    let recognition = recognize(source);
    if recognition.refine.is_some() {
        return None;
    }
    Some(
        recognition
            .cases
            .iter()
            .map(|case| {
                assert_eq!(case.certainty, Certainty::Exact, "{source}");
                case.id
                    .as_str()
                    .trim_start_matches("test_x.py::")
                    .to_string()
            })
            .collect(),
    )
}

/// A module binding `CASES`, then `setup`, then a test parametrized
/// over `CASES`.
fn over_cases(setup: &str) -> String {
    format!(
        "import pytest\n{setup}\n@pytest.mark.parametrize(\"v\", CASES)\ndef test_x(v):\n    pass\n"
    )
}

/// Module constants changed where only an import shows it (pytest
/// collects the extra or reordered parameters), each refined.
const CHANGED: &[(&str, &str)] = &[
    (
        "helper",
        "CASES = [1, 2]\ndef _add():\n    CASES.append(3)\n_add()",
    ),
    ("alias", "CASES = [1, 2]\nALIAS = CASES\nALIAS.append(3)"),
    (
        "class_alias",
        "CASES = [1, 2]\nclass H:\n    x = CASES\nH.x.append(3)",
    ),
    (
        "class_dict",
        "CASES = [1, 2]\nclass H:\n    x = CASES\nH.__dict__[\"x\"].append(3)",
    ),
    (
        "class_getattr",
        "CASES = [1, 2]\nclass H:\n    x = CASES\ngetattr(H, \"x\").append(3)",
    ),
    (
        "class_handed",
        "from elsewhere import register\nCASES = [1, 2]\nclass H:\n    x = CASES\nregister(H)",
    ),
    (
        "class_base",
        "from elsewhere import Base\nCASES = [1, 2]\nclass H(Base):\n    x = CASES",
    ),
    (
        "class_metaclass",
        "from elsewhere import Meta\nCASES = [1, 2]\nclass H(metaclass=Meta):\n    x = CASES",
    ),
    (
        "instance_default",
        "from dataclasses import dataclass\nCASES = [1, 2]\nclass P:\n    items = CASES\nP().items.append(3)",
    ),
    (
        "modules_literal",
        "import sys\nCASES = [1, 2]\nsys.modules[\"test_x\"].CASES.append(3)",
    ),
    (
        "from_itself",
        "CASES = [1, 2]\nfrom test_x import CASES as SAME\nSAME.append(3)",
    ),
    (
        "frame_back",
        "import inspect\nCASES = [1, 2]\ndef f():\n    inspect.stack()[0].frame.f_back.f_globals[\"CASES\"].append(3)\nf()",
    ),
    (
        "passed",
        "CASES = [1, 2]\ndef mutate(l):\n    l.append(3)\nmutate(CASES)",
    ),
    ("list_sort", "CASES = [3, 1, 2]\nlist.sort(CASES)"),
    (
        "type_append",
        "CASES = [1, 2]\ntype(CASES).append(CASES, 3)",
    ),
    (
        "comprehension",
        "CASES = [1, 2]\n[CASES.append(i) for i in (7, 8)]",
    ),
    (
        "if_expression",
        "CASES = [1, 2]\nCASES.append(3) if True else None",
    ),
    ("printed", "CASES = [1, 2]\nprint(CASES.append(3))"),
    ("lambda", "CASES = [1, 2]\n(lambda: CASES.append(3))()"),
    (
        "walrus",
        "CASES = [1, 2]\nif (n := CASES.append(3)) is None:\n    pass",
    ),
    ("assert", "CASES = [1, 2]\nassert CASES.append(3) is None"),
    (
        "match",
        "CASES = [1, 2]\nmatch CASES.append(3):\n    case _:\n        pass",
    ),
    (
        "with",
        "import contextlib\nCASES = [1, 2]\nwith contextlib.nullcontext(CASES.append(3)):\n    pass",
    ),
    (
        "default",
        "CASES = [1, 2]\ndef f(x=CASES.append(3)):\n    pass",
    ),
    (
        "decorator",
        "CASES = [1, 2]\ndef register(x):\n    return lambda f: f\n@register(CASES.append(3))\ndef helper():\n    pass",
    ),
    (
        "sort_if",
        "CASES = [3, 1, 2]\nCASES.sort() if CASES else None",
    ),
    ("pop", "CASES = [1, 2, 3]\n_last = CASES.pop()"),
    ("dunder", "CASES = [1, 2]\nCASES.__iadd__([9])"),
    (
        "class_body",
        "CASES = [1, 2]\nclass Helper:\n    CASES.append(3)",
    ),
    (
        "augmented_alias",
        "CASES = [1, 2]\nOTHER = CASES\nOTHER += [3]",
    ),
    ("kept_method", "CASES = [1, 2]\nadd = CASES.append\nadd(3)"),
    (
        "loop_variable",
        "CASES = [[1], [2]]\nfor row in CASES:\n    row.append(0)",
    ),
    (
        "nested_call",
        "CASES = [1, 2]\ndef a():\n    b()\ndef b():\n    CASES.append(3)\na()",
    ),
    (
        "decorated",
        "CASES = [1, 2]\ndef run(f):\n    f()\n    return f\n@run\ndef add():\n    CASES.append(3)",
    ),
    ("getattr", "CASES = [1, 2]\ngetattr(CASES, \"append\")(3)"),
    (
        "used_class",
        "CASES = [1, 2]\nclass Adder:\n    def __init__(self):\n        CASES.append(3)\nAdder()",
    ),
    (
        "hostile_dunder",
        "CASES = [1, 2]\nclass Eater:\n    def __eq__(self, other):\n        other.append(3)\n        return False\nEater() == CASES",
    ),
    (
        "globals",
        "CASES = [1, 2]\ng = globals()\ng[\"CASES\"] = [9]",
    ),
    (
        "setattr",
        "import sys\nCASES = [1, 2]\nsetattr(sys.modules[__name__], \"CASES\", [9])",
    ),
    ("exec", "CASES = [1, 2]\nexec(\"CASES.append(3)\")"),
    ("dict_update", "CASES = {\"a\": 1}\nCASES.update(b=2)"),
    (
        "stored",
        "CASES = [1, 2]\nclass Box:\n    pass\nBox.items = CASES\nBox.items.append(3)",
    ),
    (
        "local_alias",
        "CASES = [1, 2]\ndef grow():\n    items = CASES\n    items.append(3)\ngrow()",
    ),
    (
        "closure",
        "CASES = [1, 2]\ndef outer():\n    items = CASES\n    def inner():\n        items.append(3)\n    inner()\nouter()",
    ),
    (
        "default_parameter",
        "CASES = [1, 2]\ndef grow(items=CASES):\n    items.append(3)\ngrow()",
    ),
    (
        "global_statement",
        "CASES = [1, 2]\ndef grow():\n    global CASES\n    CASES = CASES + [3]\ngrow()",
    ),
];

#[test]
fn constants_changed_by_code_aye_cannot_follow_are_refined() {
    for (name, setup) in CHANGED {
        assert_eq!(exact(&over_cases(setup)), None, "{name}");
    }
    // The same through an element: `INNER` shares `CASES`'s fate.
    let nested = "import pytest\nINNER = [1, 2]\nCASES = [INNER, 3]\nCASES[0].append(5)\n@pytest.mark.parametrize(\"v\", INNER)\ndef test_x(v):\n    pass\n";
    assert_eq!(exact(nested), None);
    // And in a class body, for the module's constant.
    let class = "import pytest\nCASES = [1, 2]\nclass TestK:\n    CASES.append(3)\n    @pytest.mark.parametrize(\"v\", CASES)\n    def test_k(self, v):\n        pass\n";
    assert_eq!(exact(class), None);
    // A class attribute changed through its class.
    let attribute = "import pytest\nclass TestK:\n    CASES = [1, 2]\n    @pytest.mark.parametrize(\"v\", CASES)\n    def test_k(self, v):\n        pass\nTestK.CASES.append(3)\n";
    assert_eq!(exact(attribute), None);
    let rebound = "import pytest\nclass TestK:\n    CASES = (1, 2)\n    @pytest.mark.parametrize(\"v\", CASES)\n    def test_k(self, v):\n        pass\nTestK.CASES = (3,)\n";
    assert_eq!(exact(rebound), None);
}

/// Reads, and code that only runs after collection, keep constants
/// static.
const UNCHANGED: &[(&str, &str, &[&str])] = &[
    (
        "test_body",
        "import pytest\nCASES = [1, 2]\ndef helper(x):\n    return x\n@pytest.mark.parametrize(\"v\", CASES)\ndef test_x(v):\n    assert helper(CASES)\n    CASES.append(v)\n",
        &["test_x[1]", "test_x[2]"],
    ),
    (
        "fixture",
        "import pytest\nCASES = [1, 2]\n@pytest.fixture\ndef data():\n    return CASES\n@pytest.mark.parametrize(\"v\", CASES)\ndef test_x(v, data):\n    pass\n",
        &["test_x[1]", "test_x[2]"],
    ),
    (
        "immutable",
        "import pytest\nCASES = (1, 2)\nprint(CASES)\nsorted(CASES, key=str)\n@pytest.mark.parametrize(\"v\", CASES)\ndef test_x(v):\n    pass\n",
        &["test_x[1]", "test_x[2]"],
    ),
    (
        "reads",
        "import pytest\nCASES = [1, 2]\nN = len(CASES)\nFIRST = CASES[0]\nCOPY = list(CASES)\nTEXT = \", \".join(str(c) for c in CASES)\nassert 1 in CASES\n@pytest.mark.parametrize(\"v\", CASES)\ndef test_x(v):\n    pass\n",
        &["test_x[1]", "test_x[2]"],
    ),
    (
        "concatenation",
        "import pytest\nBASE = [1, 2]\nALL = BASE + [3]\n@pytest.mark.parametrize(\"v\", ALL)\ndef test_x(v):\n    pass\n@pytest.mark.parametrize(\"w\", BASE)\ndef test_y(w):\n    pass\n",
        &[
            "test_x[1]",
            "test_x[2]",
            "test_x[3]",
            "test_y[1]",
            "test_y[2]",
        ],
    ),
    (
        "unused_helper",
        "import pytest\nCASES = [1, 2]\ndef never_called():\n    CASES.append(3)\n@pytest.mark.parametrize(\"v\", CASES)\ndef test_x(v):\n    pass\n",
        &["test_x[1]", "test_x[2]"],
    ),
    (
        "class_attributes",
        "import pytest\nclass TestK:\n    CASES = [1, 2]\n    pytestmark = []\n    pytestmark += [pytest.mark.slow]\n    @pytest.mark.parametrize(\"v\", CASES)\n    def test_k(self, v):\n        self.CASES.append(v)\n",
        &["TestK::test_k[1]", "TestK::test_k[2]"],
    ),
    (
        "shadowing_parameter",
        "import pytest\nE = [2, 8]\ndef wrap(f):\n    return f\n@pytest.mark.parametrize(\"E\", E)\n@wrap\ndef test_x(E):\n    print(E)\n    E = []\n    E.append(1)\n",
        &["test_x[2]", "test_x[8]"],
    ),
    (
        "class_sibling",
        "import pytest\nCASES = [1, 2]\nOTHER = [3]\nclass H:\n    a = CASES\n    b = OTHER\nH.b.append(5)\nH.c = 1\n@pytest.mark.parametrize(\"v\", CASES)\ndef test_x(v):\n    pass\n",
        &["test_x[1]", "test_x[2]"],
    ),
    (
        "pytest_arguments",
        "import pytest\nCASES = [1, 2]\nIDS = [\"one\", \"two\"]\n@pytest.mark.parametrize(\"v\", CASES, ids=IDS)\ndef test_x(v):\n    pass\n@pytest.mark.parametrize(\"w\", [pytest.param(CASES, id=\"all\")])\ndef test_y(w):\n    pass\n",
        &["test_x[one]", "test_x[two]", "test_y[all]"],
    ),
];

#[test]
fn reads_and_code_run_after_collection_keep_constants_exact() {
    for (name, source, expected) in UNCHANGED {
        let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
        assert_eq!(exact(source), Some(expected), "{name}");
    }
}

#[test]
fn walrus_match_and_except_bindings_and_del_are_followed() {
    for (name, setup, values) in [
        (
            "comprehension",
            "X = [\"a\"]\n[(X := [\"comp\"]) for _ in range(1)]",
            "X",
        ),
        (
            "match",
            "X = [\"a\"]\nmatch [\"m\"]:\n    case X:\n        pass",
            "X",
        ),
        ("argument", "X = [\"a\"]\nprint(X := [\"p\"])", "X"),
        ("assigned", "X = [\"a\"]\nY = (X := [\"w\"])", "X"),
        ("test", "N = 2\nif (N := 3):\n    pass", "range(N)"),
        (
            "default",
            "V = \"orig\"\ndef f(x=(V := \"default\")):\n    pass",
            "[V]",
        ),
        ("statement", "X = [\"a\"]\n(X := [\"walrus\"])", "X"),
        (
            "except",
            "X = [\"a\"]\ntry:\n    raise ValueError\nexcept ValueError as X:\n    pass",
            "X",
        ),
        (
            "maybe_deleted",
            "import sys\nX = [\"a\"]\nif sys.platform == \"nope\":\n    del X",
            "X",
        ),
    ] {
        let source = format!(
            "import pytest\n{setup}\n@pytest.mark.parametrize(\"v\", {values})\ndef test_x(v):\n    pass\n"
        );
        assert_eq!(exact(&source), None, "{name}");
    }
    // pytest: test_x[len] (the builtin), and TestA::test_a[mod].
    assert_eq!(
        exact(
            "import pytest\nlen = 5\ndel len\n@pytest.mark.parametrize(\"v\", [len])\ndef test_x(v):\n    pass\n"
        ),
        Some(vec!["test_x[len]".to_string()])
    );
    assert_eq!(
        exact(
            "import pytest\nX = [\"mod\"]\nclass TestA:\n    X = [\"cls\"]\n    del X\n    @pytest.mark.parametrize(\"v\", X)\n    def test_a(self, v):\n        pass\n"
        ),
        Some(vec!["TestA::test_a[mod]".to_string()])
    );
}

#[test]
fn renamed_functions_and_classes_are_refined() {
    for source in [
        "import pytest\ndef renamed():\n    pass\nrenamed.__name__ = \"other_name\"\n@pytest.mark.parametrize(\"v\", [renamed])\ndef test_x(v):\n    pass\n",
        "import pytest\nclass Klass:\n    pass\nKlass.__name__ = \"NewKlass\"\n@pytest.mark.parametrize(\"v\", [Klass])\ndef test_x(v):\n    pass\n",
        "import pytest\ndef renamed():\n    pass\ndef _rename():\n    renamed.__name__ = \"via_helper\"\n_rename()\n@pytest.mark.parametrize(\"v\", [renamed])\ndef test_x(v):\n    pass\n",
        "import pytest\ndef renamed():\n    pass\ng = renamed\ng.__name__ = \"alias\"\n@pytest.mark.parametrize(\"v\", [renamed])\ndef test_x(v):\n    pass\n",
        "import pytest\nf = lambda: 0\nf.__name__ = \"lam\"\n@pytest.mark.parametrize(\"v\", [f])\ndef test_x(v):\n    pass\n",
        "import pytest\ndef renamed():\n    pass\nclass TestK:\n    renamed.__name__ = \"in_class\"\n    @pytest.mark.parametrize(\"v\", [renamed])\n    def test_k(self, v):\n        pass\n",
    ] {
        assert_eq!(exact(source), None, "{source}");
    }
    // Renamed only once the tests run.
    assert_eq!(
        exact(
            "import pytest\ndef plain():\n    pass\n@pytest.mark.parametrize(\"v\", [plain, len])\ndef test_x(v):\n    plain.__name__ = \"late\"\n"
        ),
        Some(vec!["test_x[plain]".to_string(), "test_x[len]".to_string()])
    );
}

#[test]
fn generators_are_named_genexpr() {
    assert_eq!(
        exact("import pytest\n@pytest.mark.parametrize(\"v\", [range(2), (x for x in []), ..., {1}, {}, [], ()])\ndef test_opaque(v):\n    pass\n").unwrap(),
        [
            "test_opaque[v0]",
            "test_opaque[<genexpr>]",
            "test_opaque[v2]",
            "test_opaque[v3]",
            "test_opaque[v4]",
            "test_opaque[v5]",
            "test_opaque[v6]",
        ]
    );
}

#[test]
fn complex_sums_whose_sign_depends_on_the_python_version_are_refined() {
    // Python 3.14 gives (1-0j), 3.13 (1+0j).
    for value in ["1 - 0j", "0.0 - 0j", "-0.0 - 0j", "1 + -0j", "True - 0j"] {
        let source = format!(
            "import pytest\n@pytest.mark.parametrize(\"c\", [{value}])\ndef test_c(c):\n    pass\n"
        );
        assert_eq!(exact(&source), None, "{value}");
    }
    assert_eq!(
        exact("import pytest\n@pytest.mark.parametrize(\"c\", [1 + 2j, 1 - 2j, -0.0 + 0j, 1 + 0j, 0 - -0j, (1+1j) - 0j, -3j])\ndef test_c(c):\n    pass\n").unwrap(),
        [
            "test_c[(1+2j)]",
            "test_c[(1-2j)]",
            "test_c[0j0]",
            "test_c[(1+0j)]",
            "test_c[0j1]",
            "test_c[(1+1j)]",
            "test_c[(-0-3j)]",
        ]
    );
}

#[test]
fn surrogates_in_f_strings_are_refined() {
    for value in [
        r#"f"\ud800""#,
        r#""x" f"\udc00""#,
        r#"f"\ufffd""#,
        r#"f"a{'b'}\udfff""#,
    ] {
        let source = format!(
            "import pytest\n@pytest.mark.parametrize(\"s\", [{value}])\ndef test_s(s):\n    pass\n"
        );
        assert_eq!(exact(&source), None, "{value}");
    }
    assert_eq!(
        exact("import pytest\n@pytest.mark.parametrize(\"s\", [f\"\\u00e9\", \"x\" f\"y\"])\ndef test_s(s):\n    pass\n").unwrap(),
        ["test_s[\\xe9]", "test_s[xy]"]
    );
}

#[test]
fn dict_keys_are_deduplicated_as_python_does() {
    for (keys, expected) in [
        (r#"{"a": 1, "a": 2, "b": 3}"#, &["a", "b"][..]),
        (r#"{1: "x", True: "y", 1.0: "z", 2: "w"}"#, &["1", "2"]),
        ("{0: 1, False: 2, 0j: 3, -0.0: 4}", &["0"]),
        ("{True: 1, 1: 2}", &["True"]),
        (
            r#"{3: 1, "3": 2, b"3": 3, 3.5: 4, (1+2j): 5, (1+2j): 6}"#,
            &["3_0", "3_1", "3_2", "3.5", "(1+2j)"],
        ),
    ] {
        let source = format!(
            "import pytest\n@pytest.mark.parametrize(\"k\", {keys})\ndef test_k(k):\n    pass\n"
        );
        let expected: Vec<String> = expected.iter().map(|id| format!("test_k[{id}]")).collect();
        assert_eq!(exact(&source), Some(expected), "{keys}");
    }
    // Keys that can't be compared statically.
    for keys in [
        "{float(\"nan\"): 1}",
        "{(1, 2): 1}",
        "{len: 1}",
        "{10**20: 1}",
    ] {
        let source = format!(
            "import pytest\n@pytest.mark.parametrize(\"k\", {keys})\ndef test_k(k):\n    pass\n"
        );
        assert_eq!(exact(&source), None, "{keys}");
    }
}

#[test]
fn ranges_reaching_the_limits_of_i64() {
    for (range, expected) in [
        (
            "9223372036854775806, 9223372036854775807, 2",
            "9223372036854775806",
        ),
        (
            "4611686018427387903, 4611686018427387904, 4611686018427387905",
            "4611686018427387903",
        ),
        (
            "-9223372036854775807, -9223372036854775808, -5",
            "-9223372036854775807",
        ),
    ] {
        let source = format!(
            "import pytest\n@pytest.mark.parametrize(\"v\", range({range}))\ndef test_r(v):\n    pass\n"
        );
        assert_eq!(
            exact(&source),
            Some(vec![format!("test_r[{expected}]")]),
            "{range}"
        );
    }
}

#[test]
fn values_that_raise_at_import_are_refined() {
    let huge_hex = format!("0x{}", "f".repeat(4000));
    let long_decimal = "9".repeat(700);
    let huge_int = format!("1{}", "0".repeat(400));
    let float_of_huge = format!("float({huge_int})");
    let refined = [
        r#"float("1._5")"#,
        r#"float("1_e5")"#,
        r#"float("1e_5")"#,
        r#"float("1_.5")"#,
        r#"float("+-5")"#,
        r#"float("--5")"#,
        r#"float("1e")"#,
        r#"float("١")"#,
        &float_of_huge,
        r#"re.compile("(")"#,
        r#"re.compile(r"\p")"#,
        r#"re.compile(r"a**")"#,
        r#"re.compile(r"[z-a]")"#,
        r#"re.compile("a", re.X)"#,
        r#"re.compile(b"\u00e9")"#,
        "ExceptionGroup",
        "BaseExceptionGroup",
        "PythonFinalizationError",
        &huge_hex,
        &long_decimal,
    ];
    for value in refined {
        let source = format!(
            "import pytest\nimport re\n@pytest.mark.parametrize(\"v\", [{value}])\ndef test_v(v):\n    pass\n"
        );
        assert_eq!(exact(&source), None, "{value}");
    }
    assert_eq!(
        exact("import pytest\nimport re\n@pytest.mark.parametrize(\"v\", [float(\" 1_000.25e-1_0 \"), float(\"-inf\"), float(\".5\"), float(\"5.\"), re.compile(r\"^a[b-d]+\\d{2,3}(?:x|y)?(?P<n>z)$\"), re.compile(\"a\", re.I | re.S), EnvironmentError])\ndef test_v(v):\n    pass\n").unwrap(),
        [
            "test_v[1.00025e-07]",
            "test_v[-inf]",
            "test_v[0.5]",
            "test_v[5.0]",
            "test_v[^a[b-d]+\\\\d{2,3}(?:x|y)?(?P<n>z)$]",
            "test_v[a]",
            "test_v[OSError]",
        ]
    );
}

/// Values aye reads as having no id of their own (argname + index),
/// but which pytest fails to build while it imports the module.
const RAISING: &[&str] = &[
    "range(0, 3, 0)",
    "range(1.5)",
    "range(\"a\")",
    "range()",
    "range(1, 2, 3, 4)",
    "range(0, 3, False)",
    "tuple(5)",
    "list(5)",
    "set([[1]])",
    "frozenset([[1]])",
    "{[1]: 2}",
    "{{1}}",
    "{(1, [2])}",
    "decimal.Decimal(\"x\")",
    "decimal.Decimal(\"1.2.3\")",
    "pathlib.Path(1)",
    "uuid.UUID(\"xyz\")",
    "datetime.date(2023, 2, 29)",
    "fractions.Fraction(1, 0)",
    "[*1]",
    "{**1}",
    "{**[1]}",
    "dict(1)",
    "dict([1])",
    "dict([(1, 2, 3)])",
    "slice()",
    "bytearray(\"a\")",
    "bytearray([256])",
    "object(1)",
    "(x for x in 1)",
    "lambda x=1/0: x",
    "[x for x in 5]",
    "[1] + (2,)",
    "[[1/0]]",
];

#[test]
fn values_built_by_known_calls_are_checked() {
    for value in RAISING {
        let source = format!(
            "import datetime, decimal, fractions, pathlib, uuid\nimport pytest\n@pytest.mark.parametrize(\"v\", [{value}])\ndef test_v(v):\n    pass\n"
        );
        assert_eq!(exact(&source), None, "{value}");
    }
    // Argument values themselves: a list and a tuple don't add up, and
    // a dict's values are evaluated too.
    for values in ["[1] + (2,)", "{1: 1/0}", "range(2) + [1]"] {
        let source = format!(
            "import pytest\n@pytest.mark.parametrize(\"v\", {values})\ndef test_v(v):\n    pass\n"
        );
        assert_eq!(exact(&source), None, "{values}");
    }
    // A call aye evaluates that fails at import, outside parametrize.
    assert_eq!(
        exact(
            "import pytest\nX = range(0, 3, 0)\n@pytest.mark.parametrize(\"v\", [1])\ndef test_v(v):\n    pass\n"
        ),
        None
    );
    // What pytest accepts stays exact (`constructed` in the suites
    // checks the ids against pytest).
    assert_eq!(
        exact(
            "import pytest\nX = range(3)\nY = float(\"1.5\")\n@pytest.mark.parametrize(\"v\", [range(-1), dict(a=1), {1: 2, **{3: 4}}, [*range(2)]])\ndef test_v(v):\n    pass\n"
        ),
        Some(vec![
            "test_v[v0]".to_string(),
            "test_v[v1]".to_string(),
            "test_v[v2]".to_string(),
            "test_v[v3]".to_string(),
        ])
    );
}

#[test]
fn values_of_changed_modules_are_refined() {
    // Each changes `re.compile`, which `[re.compile("ab")]` reads.
    for setup in [
        "import re as r\ndel r.compile",
        "import re\nre.compile, X = None, 1",
        "import re\n[re][0].compile = None",
        "import re\nimport helpers\nhelpers.re.compile = None",
        "import re\nimport importlib\nimportlib.import_module(\"re\").compile = None",
        "import re\n__import__(\"re\").compile = None",
        "import re\nimport sys\nsys.modules.get(\"re\").compile = None",
        "import re\nfrom mock import patch\n@patch.object(re, \"compile\")\ndef helper(m):\n    pass",
        "import re\nimport sys\nsys.modules[sys.argv[0]].compile = None",
        "import re\ndef f():\n    global re\nf()",
    ] {
        let source = format!(
            "import pytest\n{setup}\n@pytest.mark.parametrize(\"v\", [re.compile(\"ab\")])\ndef test_v(v):\n    pass\n"
        );
        assert_eq!(exact(&source), None, "{setup}");
    }
    // pytest's own API, which parametrize and marks are read through.
    assert_eq!(
        exact("import pytest\npytest.param = None\ndef test_v():\n    pass\n"),
        None
    );
    // Other attributes, other modules, and code that runs later don't
    // count.
    assert_eq!(
        exact(
            "import pytest\nimport re\nimport os\npytest.expected_config = 1\nre.extra = 2\nos.sep2 = 3\ndef later():\n    re.compile = None\n@pytest.mark.parametrize(\"v\", [re.compile(\"ab\"), *range(1)])\ndef test_v(v):\n    re.compile = None\n"
        ),
        Some(vec!["test_v[ab]".to_string(), "test_v[0]".to_string()])
    );
}

/// Every value of [`RAISING`] does make pytest's import fail.
#[test]
fn raising_values_raise_in_pytest() {
    let Some(python) = python() else { return };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pytest.ini"), "[pytest]\n").unwrap();
    for (i, value) in RAISING.iter().enumerate() {
        std::fs::write(
            dir.path().join(format!("test_r{i}.py")),
            format!(
                "import datetime, decimal, fractions, pathlib, uuid\nimport pytest\n@pytest.mark.parametrize(\"v\", [{value}])\ndef test_v(v):\n    pass\n"
            ),
        )
        .unwrap();
    }
    let output = Command::new(python)
        .args([
            "-m",
            "pytest",
            "--collect-only",
            "-q",
            "-p",
            "no:cacheprovider",
        ])
        .current_dir(dir.path())
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env_remove("PYTEST_ADDOPTS")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    for (i, value) in RAISING.iter().enumerate() {
        assert!(
            stdout.contains(&format!("ERROR test_r{i}.py")),
            "{value} doesn't raise:\n{stdout}"
        );
    }
}

/// The interpreter for differential tests, with pytest: aye-test's.
fn python() -> Option<PathBuf> {
    aye_test::test_python(aye_test::Needs::Pytest)
}

#[test]
fn static_values_match_pytest() {
    let Some(python) = python() else { return };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pytest.ini"), "[pytest]\n").unwrap();
    let mut expected = Vec::new();
    for (name, source, ids) in UNCHANGED {
        std::fs::write(dir.path().join(format!("test_{name}.py")), source).unwrap();
        expected.extend(ids.iter().map(|id| format!("test_{name}.py::{id}")));
    }
    let output = Command::new(python)
        .args([
            "-m",
            "pytest",
            "--collect-only",
            "-q",
            "-p",
            "no:cacheprovider",
        ])
        .current_dir(dir.path())
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env_remove("PYTEST_ADDOPTS")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut collected: Vec<&str> = stdout.lines().filter(|l| l.contains("::")).collect();
    collected.sort_unstable();
    expected.sort_unstable();
    assert_eq!(collected, expected, "{stdout}");
}
