use super::*;

fn exact(id: &str) -> Case {
    Case::new(id, "pytest")
}

fn partial(id: &str) -> Case {
    let mut case = Case::new(id, "pytest");
    case.certainty = Certainty::Partial;
    case
}

fn tagged(mut case: Case, tags: &[&str]) -> Case {
    case.tags = tags.iter().map(|t| (*t).to_string()).collect();
    case
}

fn ids(selected: &Selected) -> Vec<&str> {
    selected.cases.iter().map(|c| c.id.as_str()).collect()
}

fn failed(ids: &[&str]) -> Vec<TestId> {
    ids.iter().map(|id| TestId::new(*id)).collect()
}

/// A small project: two files, a class, a parametrized test collected
/// exactly and one only known at runtime.
fn project() -> Vec<Case> {
    vec![
        tagged(
            exact("tests/auth/test_api.py::TestLogin::test_ok"),
            &["slow"],
        ),
        exact("tests/auth/test_api.py::TestLogin::test_bad"),
        exact("tests/auth/test_api.py::test_logout"),
        exact("tests/test_cart.py::test_add[eur]"),
        exact("tests/test_cart.py::test_add[usd]"),
        tagged(partial("tests/test_cart.py::test_total"), &["db"]),
    ]
}

fn keyword(expr: &str) -> Selection {
    Selection::default().with_keyword(expr).unwrap()
}

fn tags(expr: &str) -> Selection {
    Selection::default().with_tags(expr).unwrap()
}

#[test]
fn the_default_selects_everything() {
    let selection = Selection::default();
    assert!(selection.is_empty());
    let selected = selection.apply(project());
    assert_eq!(selected.cases, project());
    assert_eq!(selected.deselected, 0);
    assert!(selected.narrowed.is_empty() && selected.unmatched.is_empty());
}

#[test]
fn positional_arguments_select_their_union() {
    let selected = Selection::default()
        .with_ids([
            "tests/auth/test_api.py::TestLogin",
            "tests/test_cart.py::test_add",
        ])
        .apply(project());
    assert_eq!(
        ids(&selected),
        [
            "tests/auth/test_api.py::TestLogin::test_ok",
            "tests/auth/test_api.py::TestLogin::test_bad",
            "tests/test_cart.py::test_add[eur]",
            "tests/test_cart.py::test_add[usd]",
        ]
    );
    // pytest never collects what no argument names: nothing is deselected.
    assert_eq!(selected.deselected, 0);

    let by_dir = Selection::default()
        .with_ids(["tests/auth"])
        .apply(project());
    assert_eq!(by_dir.cases.len(), 3);
    let by_word = Selection::default().with_ids(["tests"]).apply(project());
    assert_eq!(by_word.cases.len(), 6);
    let by_dotted = Selection::default()
        .with_ids(["tests.auth.test_api.TestLogin.test_bad"])
        .apply(project());
    assert_eq!(
        ids(&by_dotted),
        ["tests/auth/test_api.py::TestLogin::test_bad"]
    );
    let key = TestId::new("tests/test_cart.py::test_add[usd]")
        .key()
        .to_string();
    let by_key = Selection::default().with_ids([&key[..8]]).apply(project());
    assert_eq!(ids(&by_key), ["tests/test_cart.py::test_add[usd]"]);
}

/// Finding 83: naming one test counted its siblings as deselected.
#[test]
fn deselected_counts_only_what_the_expressions_drop() {
    let one = Selection::default()
        .with_ids(["tests/auth/test_api.py::test_logout"])
        .apply(project());
    assert_eq!(ids(&one), ["tests/auth/test_api.py::test_logout"]);
    assert_eq!(one.deselected, 0);

    let missing = Selection::default()
        .with_ids(["tests/auth/test_api.py::test_nope"])
        .apply(project());
    assert_eq!(missing.deselected, 0);
    assert_eq!(missing.unmatched, ["tests/auth/test_api.py::test_nope"]);

    let dotted = Selection::default()
        .with_ids(["tests.auth.test_api.test_logout"])
        .apply(project());
    assert_eq!(dotted.deselected, 0);

    // -k drops from what the arguments select, and only that counts.
    let by_keyword = Selection::default()
        .with_ids(["tests/auth"])
        .with_keyword("logout")
        .unwrap()
        .apply(project());
    assert_eq!(ids(&by_keyword), ["tests/auth/test_api.py::test_logout"]);
    assert_eq!(by_keyword.deselected, 2);
    // The partial case's miss is unknown, so it stays.
    assert_eq!(keyword("logout").apply(project()).deselected, 4);
}

/// Finding 28: ids came out in collection order, whatever the arguments'.
#[test]
fn cases_follow_the_arguments_order() {
    let order = |args: &[&str]| {
        let selected = Selection::default().with_ids(args).apply(project());
        assert!(selected.unmatched.is_empty(), "{:?}", selected.unmatched);
        ids(&selected)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        order(&[
            "tests/auth/test_api.py::test_logout",
            "tests/auth/test_api.py::TestLogin::test_ok",
        ]),
        [
            "tests/auth/test_api.py::test_logout",
            "tests/auth/test_api.py::TestLogin::test_ok",
        ]
    );
    assert_eq!(
        order(&[
            "tests/test_cart.py::test_add[usd]",
            "tests/test_cart.py::test_add[eur]",
        ]),
        [
            "tests/test_cart.py::test_add[usd]",
            "tests/test_cart.py::test_add[eur]",
        ]
    );
    // An id a later file argument subsumes takes the file's place, and the
    // file keeps its own order.
    assert_eq!(
        order(&[
            "tests/auth/test_api.py::test_logout",
            "tests/test_cart.py",
            "tests/auth/test_api.py",
        ]),
        [
            "tests/test_cart.py::test_add[eur]",
            "tests/test_cart.py::test_add[usd]",
            "tests/test_cart.py::test_total",
            "tests/auth/test_api.py::TestLogin::test_ok",
            "tests/auth/test_api.py::TestLogin::test_bad",
            "tests/auth/test_api.py::test_logout",
        ]
    );
    // A directory subsumes what is below it, wherever it comes.
    assert_eq!(
        order(&["tests/test_cart.py::test_total", "tests"]),
        ids(&Selection::default().apply(project()))
    );
    // Keys and dotted names take their place too.
    let key = TestId::new("tests/test_cart.py::test_add[usd]")
        .key()
        .to_string();
    assert_eq!(
        order(&[&key[..8], "tests.auth.test_api.test_logout"]),
        [
            "tests/test_cart.py::test_add[usd]",
            "tests/auth/test_api.py::test_logout",
        ]
    );
}

#[test]
fn subsumed_arguments_are_never_unmatched() {
    let selected = Selection::default()
        .with_ids([
            "tests/auth/test_api.py::test_nope",
            "tests/auth/test_api.py",
            "tests/test_cart.py::test_add[gbp]",
            "tests/test_cart.py::test_add",
            "tests/missing.py",
            "tests",
            "nowhere/t.py",
            "nowhere/t.py",
            "tests/test_cart.py::test_total[x]",
        ])
        .apply(project());
    assert_eq!(selected.cases.len(), 6);
    // A repeat is reported once.
    assert_eq!(selected.unmatched, ["nowhere/t.py"]);

    let ids_only = Selection::default()
        .with_ids([
            "t.py::TestA::test_b[1]",
            "t.py::TestA::test_b[2]",
            "t.py::TestA",
        ])
        .apply(Vec::new());
    assert_eq!(ids_only.unmatched, ["t.py::TestA"]);
    let params = Selection::default()
        .with_ids(["t.py::test_b[1]", "t.py::test_b[2]"])
        .apply(Vec::new());
    assert_eq!(params.unmatched, ["t.py::test_b[1]", "t.py::test_b[2]"]);
}

#[test]
fn unmatched_arguments_are_reported() {
    let selected = Selection::default()
        .with_ids([
            "tests/auth",
            "tests/test_cart.py::test_nope",
            "tests/test_cart.py::test_total[x]",
            "nowhere",
        ])
        .apply(project());
    assert_eq!(
        selected.unmatched,
        ["tests/test_cart.py::test_nope", "nowhere"]
    );
}

#[test]
fn a_param_id_keeps_a_partial_case_and_narrows_it() {
    let selection = Selection::default().with_ids([
        "tests/test_cart.py::test_total[b]",
        "tests/test_cart.py::test_total[a]",
        "tests/test_cart.py::test_total[a]",
        "tests/test_cart.py::test_add[eur]",
    ]);
    let total = partial("tests/test_cart.py::test_total");
    assert_eq!(selection.truth(&total), Truth::Unknown);
    let selected = selection.apply(project());
    // In the arguments' order, as pytest collects them.
    assert_eq!(
        ids(&selected),
        [
            "tests/test_cart.py::test_total",
            "tests/test_cart.py::test_add[eur]"
        ]
    );
    assert_eq!(
        selected.ids_for(&total),
        failed(&[
            "tests/test_cart.py::test_total[b]",
            "tests/test_cart.py::test_total[a]"
        ])
    );
    let add = exact("tests/test_cart.py::test_add[eur]");
    assert_eq!(selected.ids_for(&add), vec![add.id]);
}

#[test]
fn a_whole_selector_wins_over_a_narrowing_one() {
    let selected = Selection::default()
        .with_ids(["tests/test_cart.py::test_total[a]", "tests/test_cart.py"])
        .apply(project());
    assert_eq!(selected.cases.len(), 3);
    assert!(selected.narrowed.is_empty());
}

#[test]
fn keywords_match_case_insensitive_substrings() {
    let exact: Vec<Case> = project()
        .into_iter()
        .filter(|case| case.certainty == Certainty::Exact)
        .collect();
    let select = |expr: &str| keyword(expr).apply(exact.clone());
    assert_eq!(
        ids(&select("login and not Slow")),
        ["tests/auth/test_api.py::TestLogin::test_bad"]
    );
    // Directory, file and parameter ids count.
    assert_eq!(select("auth").cases.len(), 3);
    assert_eq!(select("test_api.py").cases.len(), 3);
    assert_eq!(ids(&select("EUR")), ["tests/test_cart.py::test_add[eur]"]);
    assert_eq!(ids(&select("add[u")), ["tests/test_cart.py::test_add[usd]"]);
    // Tags are keywords, as markers are in pytest.
    assert_eq!(
        ids(&select("slow")),
        ["tests/auth/test_api.py::TestLogin::test_ok"]
    );
    // A partial case is kept when its names cannot decide.
    assert_eq!(
        ids(&keyword("login and not Slow").apply(project())),
        [
            "tests/auth/test_api.py::TestLogin::test_bad",
            "tests/test_cart.py::test_total"
        ]
    );
}

#[test]
fn adapter_keywords_replace_the_fallback() {
    let mut case = exact("tests/t.py::test_a");
    case.keywords = vec!["test_a".into(), "Special".into()];
    assert_eq!(keyword("special").truth(&case), Truth::True);
    assert_eq!(keyword("tests").truth(&case), Truth::False);
}

#[test]
fn a_keyword_miss_on_a_partial_case_is_unknown() {
    let total = partial("tests/test_cart.py::test_total");
    assert_eq!(keyword("big").truth(&total), Truth::Unknown);
    assert_eq!(keyword("not big").truth(&total), Truth::Unknown);
    assert_eq!(keyword("total").truth(&total), Truth::True);
    // A name it has decides, whatever its parameters.
    assert_eq!(keyword("not total").truth(&total), Truth::False);
    assert_eq!(keyword("big and logout").truth(&total), Truth::Unknown);
    assert_eq!(keyword("big and not total").truth(&total), Truth::False);
    assert_eq!(keyword("big or total").truth(&total), Truth::True);
    assert_eq!(
        keyword("big").truth(&exact("t.py::test_total")),
        Truth::False
    );

    let selected = keyword("big").apply(project());
    assert_eq!(ids(&selected), ["tests/test_cart.py::test_total"]);
    assert!(selected.narrowed.is_empty());
}

#[test]
fn tags_match_exact_names() {
    let selected = tags("slow or db").apply(project());
    assert_eq!(
        ids(&selected),
        [
            "tests/auth/test_api.py::TestLogin::test_ok",
            "tests/test_cart.py::test_total"
        ]
    );
    let slow = tagged(exact("t.py::test_a"), &["slow"]);
    assert_eq!(tags("slo").truth(&slow), Truth::False);
    assert_eq!(tags("SLOW").truth(&slow), Truth::False);
    assert_eq!(tags("not slow").truth(&slow), Truth::False);
    assert_eq!(tags("not db").truth(&slow), Truth::True);
}

#[test]
fn tags_with_arguments_are_unknown_when_present() {
    let device = tagged(exact("t.py::test_a"), &["device"]);
    assert_eq!(tags("device(serial=\"1\")").truth(&device), Truth::Unknown);
    assert_eq!(
        tags("device(serial=\"1\") and not slow").truth(&device),
        Truth::Unknown
    );
    assert_eq!(tags("other(serial=\"1\")").truth(&device), Truth::False);
    assert_eq!(
        tags("other(serial=\"1\")").truth(&partial("t.py::test_a")),
        Truth::Unknown
    );
}

#[test]
fn a_tag_miss_on_a_partial_case_is_unknown() {
    let total = tagged(partial("t.py::test_total"), &["db"]);
    assert_eq!(tags("slow").truth(&total), Truth::Unknown);
    assert_eq!(tags("db").truth(&total), Truth::True);
    assert_eq!(tags("not db").truth(&total), Truth::False);
    assert_eq!(tags("slow and not db").truth(&total), Truth::False);
}

#[test]
fn everything_is_joined_with_and() {
    let selection = Selection::default()
        .with_ids(["tests/test_cart.py", "tests/auth/test_api.py::TestLogin"])
        .with_keyword("not bad")
        .unwrap()
        .with_tags("not slow")
        .unwrap();
    let selected = selection.apply(project());
    assert_eq!(
        ids(&selected),
        [
            "tests/test_cart.py::test_add[eur]",
            "tests/test_cart.py::test_add[usd]",
            "tests/test_cart.py::test_total",
        ]
    );
    // TestLogin's two, dropped by the expressions; test_logout was never
    // selected.
    assert_eq!(selected.deselected, 2);
    assert_eq!(selected.unmatched, [] as [std::string::String; 0]);
}

#[test]
fn deselected_by_an_expression_is_still_matched() {
    let selected = Selection::default()
        .with_ids(["tests/auth/test_api.py::test_logout"])
        .with_keyword("nothing")
        .unwrap()
        .apply(project());
    assert_eq!(selected.cases, [] as [aye_events::Case; 0]);
    assert_eq!(selected.unmatched, [] as [std::string::String; 0]);
}

#[test]
fn an_empty_keyword_expression_is_off() {
    for expr in ["", "   ", "\t \n"] {
        let selection = keyword(expr);
        assert!(selection.keyword.is_none(), "{expr:?}");
        assert!(selection.is_empty());
    }
    assert_eq!(keyword("  test_ok").keyword.unwrap().as_str(), "test_ok");
}

#[test]
fn a_blank_tag_expression_matches_nothing() {
    assert!(tags("").tags.is_none());
    let selected = tags(" ").apply(project());
    assert_eq!(selected.cases, [] as [aye_events::Case; 0]);
    assert_eq!(selected.deselected, 6);
}

#[test]
fn bad_expressions_are_usage_errors() {
    let err = Selection::default().with_keyword("  a and").unwrap_err();
    assert_eq!(
        err.to_string(),
        "wrong expression passed to '-k': a and: at column 6: \
         expected not OR left parenthesis OR identifier; got end of input"
    );
    let err = Selection::default().with_tags("slow(").unwrap_err();
    assert_eq!(
        err.to_string(),
        "wrong expression passed to '-m': slow(: at column 6: \
         expected identifier; got end of input"
    );
    let err = Selection::default()
        .with_keyword("one or foo(x=1)")
        .unwrap_err();
    assert_eq!(
        err,
        SelectError::Keyword {
            expr: "one or foo(x=1)".into(),
            error: ParseError {
                column: 8,
                message: "keyword expressions do not support call parameters".into(),
            },
        }
    );
}

/// Finding 87: the parse error was both in the message and its source, so
/// printing the chain showed it twice.
#[test]
fn a_bad_expression_says_what_is_wrong_once() {
    use std::error::Error as _;
    for err in [
        Selection::default().with_keyword("foo and").unwrap_err(),
        Selection::default().with_tags("foo and").unwrap_err(),
    ] {
        assert!(err.source().is_none());
        let text = err.to_string();
        assert_eq!(text.matches("at column 8").count(), 1, "{text}");
        assert!(text.ends_with(": foo and: at column 8: expected not OR left parenthesis OR identifier; got end of input"), "{text}");
    }
}

#[test]
fn last_failed_keeps_what_failed() {
    let selection = Selection::default().with_last_failed(failed(&[
        "tests/auth/test_api.py::test_logout",
        "tests/test_cart.py::test_add[usd]",
        "tests/gone.py::test_x",
    ]));
    assert!(!selection.is_empty());
    let selected = selection.apply(project());
    assert_eq!(
        ids(&selected),
        [
            "tests/auth/test_api.py::test_logout",
            "tests/test_cart.py::test_add[usd]"
        ]
    );
    // As pytest's summary counts them: the module-level tests of a file
    // that failed are dropped while collecting, not deselected; the tests
    // of `TestLogin` are.
    assert_eq!(selected.deselected, 2);
}

/// Findings C23 and C31: under `--lf`, pytest's summary counts as
/// deselected only what its last-failed plugin let collection keep: files
/// that held no failure are skipped, and so are the module-level tests of a
/// failing file that didn't fail, unless the file is named on the command
/// line. Checked against pytest 9.1.1's summaries.
#[test]
fn last_failed_counts_deselected_as_pytests_summary_does() {
    let c23 = || {
        vec![
            exact("tests/test_a.py::test_p[1]"),
            exact("tests/test_a.py::test_p[2]"),
            exact("tests/test_a.py::test_p[3]"),
            exact("tests/test_a.py::TestK::test_m"),
            exact("tests/test_a.py::TestK::test_n"),
            exact("tests/test_a.py::test_err"),
            exact("tests/test_a.py::test_ok"),
            exact("tests/test_b.py::test_b"),
        ]
    };
    let last = || {
        failed(&[
            "tests/test_a.py::test_p[2]",
            "tests/test_a.py::TestK::test_m",
            "tests/test_a.py::test_err",
        ])
    };
    // pytest: "2 failed, 1 deselected, 1 error".
    let selected = Selection::default().with_last_failed(last()).apply(c23());
    assert_eq!(
        ids(&selected),
        [
            "tests/test_a.py::test_p[2]",
            "tests/test_a.py::TestK::test_m",
            "tests/test_a.py::test_err"
        ]
    );
    assert_eq!(selected.deselected, 1);
    // `--lf -k "test_p or TestK"`: pytest "2 failed, 2 deselected".
    let selected = keyword("test_p or TestK")
        .with_last_failed(last())
        .apply(c23());
    assert_eq!(selected.cases.len(), 2);
    assert_eq!(selected.deselected, 2);

    let c31 = || {
        vec![
            exact("tests/test_a.py::test_a1"),
            exact("tests/test_a.py::test_a2"),
            exact("tests/test_b.py::test_b1"),
            exact("tests/test_b.py::test_b2"),
        ]
    };
    for (args, deselected) in [
        (vec![], 0),
        (vec!["tests"], 0),
        (vec!["tests/test_a.py", "tests/test_b.py"], 1),
        (vec!["tests/test_a.py"], 1),
        (
            vec!["tests/test_a.py::test_a1", "tests/test_a.py::test_a2"],
            1,
        ),
    ] {
        let selected = Selection::default()
            .with_ids(&args)
            .with_last_failed(failed(&["tests/test_a.py::test_a1"]))
            .apply(c31());
        assert_eq!(ids(&selected), ["tests/test_a.py::test_a1"], "{args:?}");
        assert_eq!(selected.deselected, deselected, "{args:?}");
    }

    // Until collection meets a file with a module-level failure, nothing is
    // filtered: a failure only inside a class keeps every test counted.
    let selected = Selection::default()
        .with_last_failed(failed(&["t.py::TestA::test_x"]))
        .apply(vec![
            exact("t.py::TestA::test_x"),
            exact("t.py::TestA::test_y"),
            exact("t.py::test_z"),
            exact("u.py::test_u"),
        ]);
    assert_eq!(ids(&selected), ["t.py::TestA::test_x"]);
    assert_eq!(selected.deselected, 3);
}

/// An adapter's own `-k` and `-m` (pytest's, from its `addopts`) apply to
/// its own cases; the command line's, to every case.
#[test]
fn an_adapters_own_expressions_apply_to_its_cases() {
    let mut doctest = exact("tests/test_cart.py::tests.test_cart.slow");
    doctest.adapter = "doctest".to_string();
    let mut cases = project();
    cases.push(doctest);
    let own = Selection::default()
        .with_own_keyword("pytest", "not test_add")
        .unwrap()
        .with_own_tags("pytest", "not slow")
        .unwrap();
    assert!(!own.is_empty());
    let selected = own.apply(cases.clone());
    assert_eq!(
        ids(&selected),
        [
            "tests/auth/test_api.py::TestLogin::test_bad",
            "tests/auth/test_api.py::test_logout",
            "tests/test_cart.py::test_total",
            "tests/test_cart.py::tests.test_cart.slow",
        ]
    );
    assert_eq!(selected.deselected, 3);
    // The command line's `-k` applies to every case instead of the
    // adapter's; the adapter's `-m` still applies to its own.
    let selected = own.clone().with_keyword("slow or ok").unwrap().apply(cases);
    assert_eq!(
        ids(&selected),
        [
            "tests/test_cart.py::test_total",
            "tests/test_cart.py::tests.test_cart.slow",
        ]
    );
    // An empty one sets nothing; a malformed one is an error to report or
    // leave to the framework.
    assert!(
        Selection::default()
            .with_own_keyword("pytest", " ")
            .unwrap()
            .is_empty()
    );
    assert!(
        Selection::default()
            .with_own_tags("pytest", "a and")
            .is_err()
    );
}

#[test]
fn deselect_prefixes_drop_and_count_their_adapters_cases() {
    let mut doctest = exact("tests/test_cart.py::tests.test_cart.add");
    doctest.adapter = "doctest".to_string();
    let mut cases = project();
    cases.push(doctest);
    let selected = Selection::default()
        .with_deselect(
            "pytest",
            [
                // A string prefix, as pytest's: `test_add` also drops
                // `test_add[eur]` and `test_add[usd]`.
                "tests/test_cart.py::test_add",
                // A `Partial` case may hold an id the framework makes
                // otherwise: the framework deselects it itself.
                "tests/test_cart.py::test_total",
                "tests/auth/test_api.py::TestLogin::test_o",
            ],
        )
        .apply(cases);
    assert_eq!(
        ids(&selected),
        [
            "tests/auth/test_api.py::TestLogin::test_bad",
            "tests/auth/test_api.py::test_logout",
            "tests/test_cart.py::test_total",
            "tests/test_cart.py::tests.test_cart.add",
        ]
    );
    assert_eq!(selected.deselected, 3);
    // Also what an explicit argument names, as in pytest.
    let selected = Selection::default()
        .with_ids(["tests/test_cart.py::test_add[eur]"])
        .with_deselect("pytest", ["tests/test_cart.py::test_add[eur]"])
        .apply(project());
    assert_eq!(selected.cases, [] as [aye_events::Case; 0]);
    assert_eq!(selected.deselected, 1);
    assert_eq!(selected.unmatched, [] as [std::string::String; 0]);
    assert!(
        !Selection::default()
            .with_deselect("pytest", ["x"])
            .is_empty()
    );
    assert!(
        Selection::default()
            .with_deselect("pytest", [""])
            .is_empty()
    );
}

#[test]
fn last_failed_with_nothing_failed_keeps_everything() {
    let selected = Selection::default().with_last_failed([]).apply(project());
    assert_eq!(selected.cases.len(), 6);
}

#[test]
fn last_failed_outside_the_selection_keeps_the_selection() {
    let selected = Selection::default()
        .with_ids(["tests/auth"])
        .with_last_failed(failed(&["tests/test_cart.py::test_add[usd]"]))
        .apply(project());
    assert_eq!(selected.cases.len(), 3);
    assert_eq!(selected.deselected, 0);
}

#[test]
fn last_failed_applies_after_the_other_filters() {
    let selected = Selection::default()
        .with_keyword("logout or add")
        .unwrap()
        .with_last_failed(failed(&[
            "tests/auth/test_api.py::TestLogin::test_ok",
            "tests/test_cart.py::test_add[eur]",
        ]))
        .apply(project());
    assert_eq!(ids(&selected), ["tests/test_cart.py::test_add[eur]"]);
}

#[test]
fn last_failed_narrows_a_partial_case_to_its_failed_params() {
    let selected = Selection::default()
        .with_last_failed(failed(&[
            "tests/test_cart.py::test_total[b]",
            "tests/test_cart.py::test_total[a]",
            "tests/test_cart.py::test_add[usd]",
        ]))
        .apply(project());
    assert_eq!(
        ids(&selected),
        [
            "tests/test_cart.py::test_add[usd]",
            "tests/test_cart.py::test_total"
        ]
    );
    assert_eq!(
        selected.narrowed,
        BTreeMap::from([(
            TestId::new("tests/test_cart.py::test_total"),
            failed(&[
                "tests/test_cart.py::test_total[a]",
                "tests/test_cart.py::test_total[b]"
            ])
        )])
    );
}

#[test]
fn last_failed_ignores_params_of_an_exact_case() {
    let selected = Selection::default()
        .with_last_failed(failed(&["t.py::test_a[1]", "t.py::test_b"]))
        .apply(vec![exact("t.py::test_a"), exact("t.py::test_b")]);
    assert_eq!(ids(&selected), ["t.py::test_b"]);
}

#[test]
fn last_failed_files_and_directories_select_what_they_contain() {
    let selected = Selection::default()
        .with_last_failed(failed(&["tests/test_cart.py"]))
        .apply(project());
    assert_eq!(selected.cases.len(), 3);
    assert!(selected.narrowed.is_empty());
    let selected = Selection::default()
        .with_last_failed(failed(&["tests/auth"]))
        .apply(project());
    assert_eq!(selected.cases.len(), 3);
    let selected = Selection::default()
        .with_last_failed(failed(&["tests/au", "tests/test_cart.py::test_add[eur]"]))
        .apply(project());
    assert_eq!(ids(&selected), ["tests/test_cart.py::test_add[eur]"]);
}

#[test]
fn last_failed_meets_explicit_narrowing() {
    let total = partial("t.py::test_total");
    let cases = || vec![total.clone(), exact("t.py::test_other")];
    let wanted = || Selection::default().with_ids(["t.py::test_total[a]", "t.py::test_total[b]"]);

    let selected = wanted()
        .with_last_failed(failed(&["t.py::test_total[b]", "t.py::test_total[c]"]))
        .apply(cases());
    assert_eq!(selected.ids_for(&total), failed(&["t.py::test_total[b]"]));

    // None of the wanted parameters failed: nothing selected failed, so
    // everything selected runs.
    let selected = wanted()
        .with_last_failed(failed(&["t.py::test_total[c]"]))
        .apply(cases());
    assert_eq!(
        selected.ids_for(&total),
        failed(&["t.py::test_total[a]", "t.py::test_total[b]"])
    );

    // The whole test failed: every wanted parameter runs.
    let selected = wanted()
        .with_last_failed(failed(&["t.py::test_total", "t.py::test_other"]))
        .apply(cases());
    assert_eq!(selected.cases.len(), 1);
    assert_eq!(
        selected.ids_for(&total),
        failed(&["t.py::test_total[a]", "t.py::test_total[b]"])
    );
}

#[test]
fn a_selection_is_built_once_and_applied_to_many() {
    let selection = Selection {
        ids: vec![Selector::parse("tests/auth")],
        keyword: Some(Expression::parse("ok").unwrap()),
        ..Selection::default()
    };
    for _ in 0..2 {
        assert_eq!(
            ids(&selection.apply(project())),
            ["tests/auth/test_api.py::TestLogin::test_ok"]
        );
    }
}
