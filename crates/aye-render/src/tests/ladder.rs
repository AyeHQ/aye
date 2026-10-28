//! The detail ladder (see the crate's docs): what each level of each
//! format shows of a bad outcome, with every field the engine fills
//! ([`streams::full`]: the mockup's project, and more shapes).

use std::collections::BTreeMap;

use aye_events::{Event, Record};

use super::{feed, lines, streams};
use crate::{ColorChoice, JsonLines, Keys, ShowContext, ShowMode, Text, TextOptions};

fn text(records: &[Record], verbosity: i8) -> String {
    let mut text = Text::new(
        Vec::new(),
        TextOptions {
            verbosity,
            color: ColorChoice::Never,
            live: false,
        },
    );
    feed(&mut text, records);
    String::from_utf8(text.into_inner()).unwrap()
}

fn json(records: &[Record], verbosity: i8) -> String {
    let mut json = JsonLines::new(Vec::new(), verbosity);
    feed(&mut json, records);
    String::from_utf8(json.into_inner()).unwrap()
}

#[test]
fn text_l1_a_line_per_bad_outcome_then_the_closing_lines() {
    assert_eq!(
        text(&streams::full(), 0),
        lines(&[
            "aye · tests_pytest · python 3.13.16 · 19 tests · 1 worker",
            "",
            "✗ 00ef7  tests/test_cart.py::TestCart::test_fails_summary_dict",
            "         cart.summary()['pear']: expected 3, actual 2 at tests/test_cart.py:34",
            "✗ 44e77  tests/test_checkout.py::TestCheckout::test_fails_patch_wrong_binding",
            "         receipt.issued_at: expected 2026-10-11T09:30:00, actual 2026-10-15T18:40:00 at tests/test_checkout.py:64",
            "! 4f22e  tests/test_setup_errors.py::test_errors_in_setup",
            "         fixture unreachable_database: ConnectionError: database unreachable at tests/conftest.py:12",
            "! 91b8f  tests/test_setup_errors.py::test_errors_in_teardown",
            "         passed; fixture fragile_teardown failed in teardown: RuntimeError: teardown failed at tests/conftest.py:20",
            "! e793d  tests/test_setup_errors.py::test_errors_missing_fixture (setup)",
            "         fixture 'kart' not found; did you mean 'cart'? at tests/test_setup_errors.py:30",
            "✗ 7a6ea  tests/test_inventory.py::test_restock_levels",
            "         list differs at 2 of 5 positions; 1 item missing: [1], [3], [5] at tests/test_inventory.py:41",
            "✗ 20514  tests/test_receipt.py::test_receipt_text",
            "         receipt.text() (line 2, column 11): expected 'Total: 9.45', actual 'Total: 9.44' at tests/test_receipt.py:18",
            "✗ c4130  tests/test_cart.py::test_totals_agree",
            "         cart.total() is Decimal('9.44'), invoice.total() is Decimal('9.45') at tests/test_cart.py:52",
            "✗ bc4c1  tests/test_cart.py::test_line_item",
            "         cart.lines[0].quantity: expected 3, actual 2 at tests/test_cart.py:61",
            "! 46099  tests/test_api.py::test_fetch",
            "         requests.exceptions.ConnectionError: HTTPConnectionPool(host='shop.test', port=80): Max retries exceeded at src/shop/client.py:20",
            "! a3c86  tests/test_api.py::test_fetch_again",
            "         same as 46099",
            "",
            "warnings  1 of plugin-not-loaded",
            "skipped  needs GPU (2) · Linux only (1)",
            "✗ 6 failed · ! 5 errors · ✓ 5 passed · 3 skipped   2.12 s",
            "details  aye show 00ef7 44e77 4f22e  ·  every failure: aye show --failed",
        ])
    );
}

#[test]
fn text_l2_a_block_per_bad_outcome() {
    assert_eq!(
        text(&streams::full(), 1),
        lines(&[
            "aye · tests_pytest · python 3.13.16 · 19 tests · 1 worker",
            "",
            "✗ 00ef7  tests/test_cart.py::TestCart::test_fails_summary_dict",
            "  failed in the test · 2 ms",
            "",
            "  tests/test_cart.py:34",
            r#"     32 │ cart.add("apple", price_list["apple"], 4)"#,
            r#"     33 │ cart.add("pear", price_list["pear"], 2)"#,
            r#"  →  34 │ assert cart.summary() == {"apple": 4, "pear": 3}"#,
            "        │        ^^^^^^^^^^^^^^",
            "  dict differs in 1 of 2 keys",
            "    ['pear']   expected 3   actual 2",
            "",
            "  setup    price_list (session) → cart (function)",
            "  history  passed the previous 12 runs",
            "  rerun    aye tests/test_cart.py::TestCart::test_fails_summary_dict",
            "  more     aye show 00ef7 -vv   (every frame, output, locals)",
            "",
            "✗ 44e77  tests/test_checkout.py::TestCheckout::test_fails_patch_wrong_binding",
            "  failed in the test · 3 ms",
            "",
            "  tests/test_checkout.py:64",
            r#"     62 │ with patch("shop.clock.now", return_value=FROZEN_NOW):"#,
            "     63 │     receipt = checkout(cart)",
            "  →  64 │ assert receipt.issued_at == FROZEN_NOW",
            "        │        ^^^^^^^^^^^^^^^^^",
            "  datetime differs by 4 days, 9:10:00",
            "    expected 2026-10-11T09:30:00   actual 2026-10-15T18:40:00",
            "",
            "  history  failed the previous 3 runs too (since r78)",
            "  rerun    aye tests/test_checkout.py::TestCheckout::test_fails_patch_wrong_binding",
            "  more     aye show 44e77 -vv   (every frame, output, locals)",
            "",
            "! 4f22e  tests/test_setup_errors.py::test_errors_in_setup",
            "  error in setup · 1 ms",
            "",
            "  tests/conftest.py:12",
            "     10 │ @pytest.fixture",
            "     11 │ def unreachable_database():",
            r#"  →  12 │     raise ConnectionError("database unreachable")"#,
            "        │     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^",
            "  ConnectionError: database unreachable",
            "",
            "  fixture  unreachable_database",
            "  setup    unreachable_database (function)",
            "  history  failed 2 of the last 10 runs",
            "  rerun    aye tests/test_setup_errors.py::test_errors_in_setup",
            "  more     aye show 4f22e -vv   (every frame, output, locals)",
            "",
            "! 91b8f  tests/test_setup_errors.py::test_errors_in_teardown",
            "  error in teardown · 1 ms",
            "",
            "  tests/conftest.py:20",
            "     19 │ yield",
            r#"  →  20 │ raise RuntimeError("teardown failed")"#,
            "        │ ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^",
            "  RuntimeError: teardown failed",
            "",
            "  fixture  fragile_teardown",
            "  setup    fragile_teardown (function)",
            "  rerun    aye tests/test_setup_errors.py::test_errors_in_teardown",
            "  more     aye show 91b8f -vv   (every frame, output, locals)",
            "",
            "! e793d  tests/test_setup_errors.py::test_errors_missing_fixture",
            "  error in setup · 0 ms",
            "",
            "  tests/test_setup_errors.py:30",
            "  →  30 │ def test_errors_missing_fixture(kart):",
            "  fixture 'kart' not found",
            "",
            "  hint     did you mean 'cart'?",
            "  rerun    aye tests/test_setup_errors.py::test_errors_missing_fixture",
            "  more     aye show e793d -vv   (every frame, output, locals)",
            "",
            "✗ 7a6ea  tests/test_inventory.py::test_restock_levels",
            "  failed in the test · 1 ms",
            "",
            "  tests/test_inventory.py:41",
            r#"     40 │ inventory.restock("pear", 1)"#,
            "  →  41 │ assert inventory.levels() == [1, 3, 3, 5, 5, 6]",
            "        │        ^^^^^^^^^^^^^^^^^^",
            "  list differs at 2 of 5 positions; 1 item missing",
            "    [1]   expected 3   actual 2",
            "    [3]   expected 5   actual 4",
            "    [5]   expected 6   actual missing",
            "",
            "  rerun    aye tests/test_inventory.py::test_restock_levels",
            "  more     aye show 7a6ea -vv   (every frame, output, locals)",
            "",
            "✗ 20514  tests/test_receipt.py::test_receipt_text",
            "  failed in the test · 1 ms",
            "",
            "  tests/test_receipt.py:18",
            "  →  18 │ assert receipt.text() == EXPECTED_RECEIPT",
            "        │        ^^^^^^^^^^^^^^",
            "  strings differ at line 2, column 11",
            "    expected  'Total: 9.45'",
            "    actual    'Total: 9.44'",
            "",
            "  rerun    aye tests/test_receipt.py::test_receipt_text",
            "  more     aye show 20514 -vv   (every frame, output, locals)",
            "",
            "✗ c4130  tests/test_cart.py::test_totals_agree",
            "  failed in the test · 1 ms",
            "",
            "  tests/test_cart.py:52",
            "  →  52 │ assert cart.total() == invoice.total()",
            "        │        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^",
            "  differ by 0.01",
            "    cart.total() Decimal('9.44')   invoice.total() Decimal('9.45')",
            "",
            "  rerun    aye tests/test_cart.py::test_totals_agree",
            "  more     aye show c4130 -vv   (every frame, output, locals)",
            "",
            "✗ bc4c1  tests/test_cart.py::test_line_item",
            "  failed in the test · 1 ms",
            "",
            "  tests/test_cart.py:61",
            r#"  →  61 │ assert cart.lines[0] == LineItem("pear", Decimal("1.50"), 3)"#,
            "        │        ^^^^^^^^^^^^^",
            "  LineItem differs in 1 of 3 fields",
            "    .quantity   expected 3   actual 2",
            "",
            "  rerun    aye tests/test_cart.py::test_line_item",
            "  more     aye show bc4c1 -vv   (every frame, output, locals)",
            "",
            "! 46099  tests/test_api.py::test_fetch",
            "  error in the test · 2.00 s",
            "",
            "  src/shop/client.py:20",
            "     18 │ def get(self, path):",
            r#"     19 │     """Fetch one page of the API.""""#,
            "  →  20 │     return requests.get(self.base + path, timeout=2)",
            "        │            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^",
            "  requests.exceptions.ConnectionError: HTTPConnectionPool(host='shop.test', port=80): Max retries exceeded",
            "",
            "  rerun    aye tests/test_api.py::test_fetch",
            "  more     aye show 46099 -vv   (every frame, output, locals)",
            "",
            "! a3c86  tests/test_api.py::test_fetch_again",
            "         same as 46099",
            "",
            "warnings  1 of plugin-not-loaded",
            "skipped  needs GPU (2) · Linux only (1)",
            "✗ 6 failed · ! 5 errors · ✓ 5 passed · 3 skipped   2.12 s",
            "details  aye show 00ef7 44e77 4f22e  ·  every failure: aye show --failed",
        ])
    );
}

#[test]
fn text_l3_adds_frames_output_and_locals() {
    let out = text(&streams::full(), 2);
    assert!(
        out.contains(&lines(&[
            "✗ 00ef7  tests/test_cart.py::TestCart::test_fails_summary_dict",
            "  failed in the test · 2 ms",
            "",
            "  tests/test_cart.py:34",
            r#"     32 │ cart.add("apple", price_list["apple"], 4)"#,
            r#"     33 │ cart.add("pear", price_list["pear"], 2)"#,
            r#"  →  34 │ assert cart.summary() == {"apple": 4, "pear": 3}"#,
            "        │        ^^^^^^^^^^^^^^",
            "  dict differs in 1 of 2 keys",
            "    ['pear']   expected 3   actual 2",
            "",
            "  frames   tests/test_cart.py:34 in test_fails_summary_dict",
            r#"             assert cart.summary() == {"apple": 4, "pear": 3}"#,
            "  stdout   adding apple",
            "           adding pear",
            "  locals   cart = Cart(items={'apple': 4, 'pear': 2})",
            "           price_list = {'apple': Decimal('0.50'), 'pear': Decimal('0.75')}",
            "",
            "  setup    price_list (session) → cart (function)",
            "  history  passed the previous 12 runs",
            "  rerun    aye tests/test_cart.py::TestCart::test_fails_summary_dict",
        ])),
        "{out}"
    );
    // Frames outside the project, collapsed by package.
    assert!(
        out.contains(&lines(&[
            "! 46099  tests/test_api.py::test_fetch",
            "  error in the test · 2.00 s",
            "",
            "  src/shop/client.py:20",
            "     18 │ def get(self, path):",
            r#"     19 │     """Fetch one page of the API.""""#,
            "  →  20 │     return requests.get(self.base + path, timeout=2)",
            "        │            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^",
            "  requests.exceptions.ConnectionError: HTTPConnectionPool(host='shop.test', port=80): Max retries exceeded",
            "",
            "  frames   tests/test_api.py:8 in test_fetch",
            r#"             items = client.get("/items")"#,
            "           src/shop/client.py:20 in get",
            "             return requests.get(self.base + path, timeout=2)",
            "           5 frames in requests",
            "  log      WARNING urllib3: Retrying (Retry(total=0)) after connection broken",
            "  locals   path = '/items'",
            "           self = <Client base='http://shop.test'>",
            "",
            "  rerun    aye tests/test_api.py::test_fetch",
        ])),
        "{out}"
    );
    // Nothing grouped: the second request failure has its block.
    assert!(!out.contains("same as"), "{out}");
}

#[test]
fn json_l1_a_line_per_bad_outcome() {
    assert_eq!(
        json(&streams::full(), 1),
        lines(&[
            r#"{"type":"fail","key":"00ef7","id":"tests/test_cart.py::TestCart::test_fails_summary_dict","outcome":"failed","at":"tests/test_cart.py:34","why":"cart.summary()['pear']: expected 3, actual 2"}"#,
            r#"{"type":"fail","key":"44e77","id":"tests/test_checkout.py::TestCheckout::test_fails_patch_wrong_binding","outcome":"failed","at":"tests/test_checkout.py:64","why":"receipt.issued_at: expected 2026-10-11T09:30:00, actual 2026-10-15T18:40:00"}"#,
            r#"{"type":"fail","key":"4f22e","id":"tests/test_setup_errors.py::test_errors_in_setup","outcome":"error","phase":"setup","at":"tests/conftest.py:12","why":"fixture unreachable_database: ConnectionError: database unreachable"}"#,
            r#"{"type":"fail","key":"91b8f","id":"tests/test_setup_errors.py::test_errors_in_teardown","outcome":"error","phase":"teardown","at":"tests/conftest.py:20","why":"passed; fixture fragile_teardown failed in teardown: RuntimeError: teardown failed"}"#,
            r#"{"type":"fail","key":"e793d","id":"tests/test_setup_errors.py::test_errors_missing_fixture","outcome":"error","phase":"setup","at":"tests/test_setup_errors.py:30","why":"fixture 'kart' not found; did you mean 'cart'?"}"#,
            r#"{"type":"fail","key":"7a6ea","id":"tests/test_inventory.py::test_restock_levels","outcome":"failed","at":"tests/test_inventory.py:41","why":"list differs at 2 of 5 positions; 1 item missing: [1], [3], [5]"}"#,
            r#"{"type":"fail","key":"20514","id":"tests/test_receipt.py::test_receipt_text","outcome":"failed","at":"tests/test_receipt.py:18","why":"receipt.text() (line 2, column 11): expected 'Total: 9.45', actual 'Total: 9.44'"}"#,
            r#"{"type":"fail","key":"c4130","id":"tests/test_cart.py::test_totals_agree","outcome":"failed","at":"tests/test_cart.py:52","why":"cart.total() is Decimal('9.44'), invoice.total() is Decimal('9.45')"}"#,
            r#"{"type":"fail","key":"bc4c1","id":"tests/test_cart.py::test_line_item","outcome":"failed","at":"tests/test_cart.py:61","why":"cart.lines[0].quantity: expected 3, actual 2"}"#,
            r#"{"type":"fail","key":"46099","id":"tests/test_api.py::test_fetch","outcome":"error","at":"src/shop/client.py:20","why":"requests.exceptions.ConnectionError: HTTPConnectionPool(host='shop.test', port=80): Max retries exceeded"}"#,
            r#"{"type":"fail","key":"a3c86","id":"tests/test_api.py::test_fetch_again","outcome":"error","same_as":"46099","at":"src/shop/client.py:20"}"#,
            r#"{"type":"summary","run":"r81","exit":1,"tests":19,"failed":6,"errors":5,"passed":5,"skipped":3,"s":2.12,"failures":["00ef7","44e77","4f22e","91b8f","e793d","7a6ea","20514","c4130","bc4c1","46099","a3c86"],"more":"aye show <key> --output json"}"#,
        ])
    );
}

#[test]
fn json_l2_adds_the_pointed_code_and_the_structured_diff() {
    let out = json(&streams::full(), 2);
    let out: Vec<&str> = out.lines().collect();
    // A dict compared with a literal: expected and actual.
    assert!(out.contains(&r#"{"type":"fail","key":"00ef7","id":"tests/test_cart.py::TestCart::test_fails_summary_dict","outcome":"failed","at":"tests/test_cart.py:34","why":"cart.summary()['pear']: expected 3, actual 2","code":"assert cart.summary() == {\"apple\": 4, \"pear\": 3}","cols":[7,21],"before":["cart.add(\"apple\", price_list[\"apple\"], 4)","cart.add(\"pear\", price_list[\"pear\"], 2)"],"diff":{"kind":"dict","summary":"dict differs in 1 of 2 keys","same":1,"differs":[{"at":"['pear']","expected":"3","actual":"2"}]},"setup":["price_list:session","cart:function"],"history":{"previous":12,"passed":12},"rerun":"aye tests/test_cart.py::TestCart::test_fails_summary_dict","ms":2}"#), "{out:#?}");
    // A list: three differences, one an entry only the expected side has.
    assert!(out.contains(&r#"{"type":"fail","key":"7a6ea","id":"tests/test_inventory.py::test_restock_levels","outcome":"failed","at":"tests/test_inventory.py:41","why":"list differs at 2 of 5 positions; 1 item missing: [1], [3], [5]","code":"assert inventory.levels() == [1, 3, 3, 5, 5, 6]","cols":[7,25],"before":["inventory.restock(\"pear\", 1)"],"diff":{"kind":"list","summary":"list differs at 2 of 5 positions; 1 item missing","same":3,"differs":[{"at":"[1]","expected":"3","actual":"2"},{"at":"[3]","expected":"5","actual":"4"},{"at":"[5]","expected":"6"}]},"rerun":"aye tests/test_inventory.py::test_restock_levels","ms":1}"#), "{out:#?}");
    // Neither side literal: the sources name the operands.
    assert!(out.contains(&r#"{"type":"fail","key":"c4130","id":"tests/test_cart.py::test_totals_agree","outcome":"failed","at":"tests/test_cart.py:52","why":"cart.total() is Decimal('9.44'), invoice.total() is Decimal('9.45')","code":"assert cart.total() == invoice.total()","cols":[7,38],"diff":{"kind":"number","summary":"differ by 0.01"},"left_source":"cart.total()","right_source":"invoice.total()","left":"Decimal('9.44')","right":"Decimal('9.45')","rerun":"aye tests/test_cart.py::test_totals_agree","ms":1}"#), "{out:#?}");
    // A broken fixture.
    assert!(out.contains(&r#"{"type":"fail","key":"4f22e","id":"tests/test_setup_errors.py::test_errors_in_setup","outcome":"error","phase":"setup","at":"tests/conftest.py:12","why":"fixture unreachable_database: ConnectionError: database unreachable","code":"    raise ConnectionError(\"database unreachable\")","cols":[4,49],"before":["@pytest.fixture","def unreachable_database():"],"fixture":"unreachable_database","setup":["unreachable_database:function"],"history":{"previous":10,"passed":8,"failed":2},"rerun":"aye tests/test_setup_errors.py::test_errors_in_setup","ms":1}"#), "{out:#?}");
}

#[test]
fn json_l3_adds_frames_output_and_locals() {
    let out = json(&streams::full(), 3);
    assert!(
        out.lines().any(|line| line == r#"{"type":"fail","key":"46099","id":"tests/test_api.py::test_fetch","outcome":"error","at":"src/shop/client.py:20","why":"requests.exceptions.ConnectionError: HTTPConnectionPool(host='shop.test', port=80): Max retries exceeded","code":"    return requests.get(self.base + path, timeout=2)","cols":[11,52],"before":["def get(self, path):","    \"\"\"Fetch one page of the API.\"\"\""],"rerun":"aye tests/test_api.py::test_fetch","ms":2004,"frames":["tests/test_api.py:8 in test_fetch: items = client.get(\"/items\")","src/shop/client.py:20 in get: return requests.get(self.base + path, timeout=2)","5 frames in requests"],"output":{"log":"WARNING urllib3: Retrying (Retry(total=0)) after connection broken"},"locals":{"path":"'/items'","self":"<Client base='http://shop.test'>"}}"#),
        "{out}"
    );
}

/// The records `aye show <key>` feeds for one test: the run's start, the
/// test's worker, the test, and a summary counting it.
fn shown(id: &str) -> Vec<Record> {
    let records = streams::full();
    let mut part: Vec<Record> = records
        .iter()
        .filter(|record| match &record.event {
            Event::RunStarted { .. } | Event::WorkerStarted { .. } => true,
            Event::TestFinished(result) => result.id.as_str() == id,
            _ => false,
        })
        .cloned()
        .collect();
    part.push(Record::new(Event::RunFinished {
        exit_code: aye_events::ExitCode::TestsFailed,
        counts: aye_events::Counts {
            failed: 1,
            ..aye_events::Counts::default()
        },
        duration: std::time::Duration::from_millis(2),
        stopped: None,
    }));
    part
}

fn context() -> ShowContext {
    ShowContext {
        run: Some(81),
        ago: Some("2 min ago".into()),
        stale: BTreeMap::new(),
    }
}

#[test]
fn aye_show_a_key_is_its_detail_with_where_the_run_was() {
    let part = shown("tests/test_cart.py::TestCart::test_fails_summary_dict");
    let keys = Keys::of(&streams::full());
    let mut text = Text::new(
        Vec::new(),
        TextOptions {
            verbosity: 1,
            color: ColorChoice::Never,
            live: false,
        },
    )
    .for_show(ShowMode::Details, keys.clone(), context());
    feed(&mut text, &part);
    assert_eq!(
        String::from_utf8(text.into_inner()).unwrap(),
        lines(&[
            "✗ 00ef7  tests/test_cart.py::TestCart::test_fails_summary_dict",
            "  failed in the test · 2 ms · python 3.13.16 · run r81, 2 min ago · file unchanged",
            "",
            "  tests/test_cart.py:34",
            "     32 │ cart.add(\"apple\", price_list[\"apple\"], 4)",
            "     33 │ cart.add(\"pear\", price_list[\"pear\"], 2)",
            "  →  34 │ assert cart.summary() == {\"apple\": 4, \"pear\": 3}",
            "        │        ^^^^^^^^^^^^^^",
            "  dict differs in 1 of 2 keys",
            "    ['pear']   expected 3   actual 2",
            "",
            "  setup    price_list (session) → cart (function)",
            "  history  passed the previous 12 runs",
            "  rerun    aye tests/test_cart.py::TestCart::test_fails_summary_dict",
            "  more     aye show 00ef7 -vv   (every frame, output, locals)",
            "",
        ])
    );
    let mut json = JsonLines::new(Vec::new(), 2).for_show(ShowMode::Details, keys, context());
    feed(&mut json, &part);
    assert_eq!(
        String::from_utf8(json.into_inner()).unwrap(),
        lines(&[
            r#"{"type":"fail","key":"00ef7","id":"tests/test_cart.py::TestCart::test_fails_summary_dict","outcome":"failed","at":"tests/test_cart.py:34","why":"cart.summary()['pear']: expected 3, actual 2","run":"r81","python":"3.13.16","stale":false,"code":"assert cart.summary() == {\"apple\": 4, \"pear\": 3}","cols":[7,21],"before":["cart.add(\"apple\", price_list[\"apple\"], 4)","cart.add(\"pear\", price_list[\"pear\"], 2)"],"diff":{"kind":"dict","summary":"dict differs in 1 of 2 keys","same":1,"differs":[{"at":"['pear']","expected":"3","actual":"2"}]},"setup":["price_list:session","cart:function"],"history":{"previous":12,"passed":12},"rerun":"aye tests/test_cart.py::TestCart::test_fails_summary_dict","ms":2}"#,
            r#"{"type":"summary","run":"r81","exit":1,"failed":1,"s":0.0,"failures":["00ef7"]}"#,
        ])
    );
}

#[test]
fn the_hint_and_the_colours_of_the_comparison() {
    let mut text = Text::new(
        Vec::new(),
        TextOptions {
            verbosity: 1,
            color: ColorChoice::Always,
            live: false,
        },
    );
    feed(&mut text, &streams::full());
    let out = String::from_utf8(text.into_inner()).unwrap();
    // The span red and underlined, the carets red.
    assert!(
        out.contains("assert \u{1b}[4m\u{1b}[31mcart.summary()\u{1b}[0m == "),
        "{out:?}"
    );
    // Expected green, actual red.
    assert!(
        out.contains("\u{1b}[32m3\u{1b}[0m   \u{1b}[2mactual\u{1b}[0m \u{1b}[31m2\u{1b}[0m"),
        "{out:?}"
    );
}
