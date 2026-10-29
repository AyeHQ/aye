"""The pytest adapter's executor, on aye's own engine.

The worker installs this module as ``aye_pytest_native``, last, after
``aye_model``, ``aye_rewrite``, ``aye_native_engine``, aye's ``pytest``
module and ``aye_pytest_frontend``; the runner names
``aye_pytest_native:NativeExecutor`` as the pytest adapter's entry. It is
glue only: the front-end reads the suite into aye's model, the engine runs
the model, and nothing here imports pytest's own code.

- **One session per worker**, as a pytest-xdist worker has: one
  front-end lists the session's files once (``Frontend.extend``: imported
  once, the ``pytest_collection_modifyitems`` hooks run once over what it
  lists) and keeps the model; one engine runs every batch, its session and
  package lifetimes alive from batch to batch (``keep_session``) until the
  session ends. The batch the runner says is the worker's last
  (``Batch.ends_session``) ends it, as a pytest-xdist worker ends its
  session with its last test: the session's lifetimes close in its last
  test's teardown (their errors are that test's, as under pytest), then
  ``pytest_sessionfinish``, ``pytest_unconfigure`` and the cleanups run and
  report into the batch (``_end``): what fails there is a diagnostic, a
  lifetime left open with no test to blame (a batch with no ids, which only
  ends the session) is a ``lifetime_finished`` of its own and fails the
  batch, and a ``session.exitstatus`` that ``pytest_sessionfinish`` set to 1
  is the batch's status, with a ``session-failed`` warning. A session no
  batch ended (a stopped run) ends when the worker closes the executor
  (``close``), with no batch to report into: what fails then is written to
  stderr. A context
  that differs in more than what only a batch carries (``BATCH_KEYS``)
  ends the session and starts another. An ended session takes assertion
  rewriting's import hook out, as pytest's configuration does, and lets go
  of what aye made for it (the items' cases, the engine), so that none of
  it outlives the session through an item the suite's code still holds.
  ``selected_paths`` is not one of ``BATCH_KEYS``: the paths a session
  starts from decide its start-up conftests, its initial paths and
  ``config.args``, so a batch that names others gets its own session.
- Listing's own work (the items, their cases, their wire format) runs
  without automatic garbage collection (``no_automatic_gc``: no code of
  the suite runs meanwhile), then one collection of the younger
  generations settles what it made in the oldest (``settle_gc``).
- ``refine``: lists the files (those not listed yet) and returns their
  cases in the pytest adapter's wire format (``certainty: refined``), in
  listing order, those a hook drops included (refinement predicts a
  superset), with the diagnostics that concern them, and ``digests``: per
  file, the SHA-1 of its ids in order, joined by newlines. With
  ``digests_only`` (a lane listing the session before its first batch),
  the digests alone. The front-end's ``affinity`` says which cases must
  share a batch: only those sharing a costly module or class lifetime.
- ``run``: lists the batch's files not listed yet, then keeps the batch's
  ids among the listed cases the hooks kept, in the batch's order (as the
  pytest adapter's ``_select`` does: an id without parameters selects them
  all, a parametrized id pytest no longer makes stands for the parameters
  no other requested id claims unless the user typed it, and the tests
  that ran before a crash, the context's ``ran_ids``, don't run again).
  Requested ids in a module or directory pytest skips finish as
  ``skipped``; those it can't find are ``not-found``; those in a file that
  failed to collect are its diagnostic's, which the batch reports with the
  session's. The engine then runs the cases, with the front-end's policy,
  hooks and builtins, and a bad outcome's ``reproduce`` is aye's rerun
  command. A conftest's ``session.shouldstop``/``shouldfail`` stops the
  batch it is set in, not the next ones.
- The status is pytest's: 1 when a test failed or one of the batch's files
  failed to collect (pytest's ``--continue-on-collection-errors``), 5 when
  no test ran, 4 after a usage error (a conftest aye can't load), 3 after a
  hook failed outside a test.
- Assertion rewriting is installed before the first conftest is imported,
  for the modules pytest rewrites (``Frontend.should_rewrite``). A failed
  comparison's message is pytest's (``assertrepr_compare``'s summary, each
  side cut to fit), so a failure reads, and is signed, as the pytest
  adapter reports it; the engine's explanation is ``assertion.diff``.
- Session-wide warnings refinement reports (``REFINEMENT_WARNINGS``) are
  not repeated by batches.
- ``PYTEST_VERSION`` is set while the executor lists, runs and closes, as
  pytest sets it, so code that checks it knows it runs under pytest.

Standard library only (Python 3.10+).
"""

from __future__ import annotations

import json
import os
import sys
from collections import Counter
from collections.abc import Callable, Iterable, Iterator, Sequence
from contextlib import contextmanager
from hashlib import sha1
from typing import Any
from unicodedata import normalize

import aye_engine
import aye_native_engine
import aye_pytest_frontend
import aye_rewrite
import pytest

# pytest's exit statuses.
OK = 0
TESTS_FAILED = 1
INTERRUPTED = 2
INTERNAL_ERROR = 3
USAGE_ERROR = 4
NO_TESTS_COLLECTED = 5

# A diagnostic names at most this many tests.
MAX_NAMED = 20

JUNITXML = frozenset({"--junitxml", "--junit-xml"})

# Session-wide warnings refinement reports, once per run (every run
# refines the files it runs, and a refinement with a session-wide
# warning is never cached): batches don't repeat them.
REFINEMENT_WARNINGS = frozenset({"plugin-not-loaded"})

# The context's keys only a batch carries, which don't make another session.
BATCH_KEYS = frozenset({"requested_ids", "ran_ids", "rerun_args"})


def _register_engine_types() -> None:
    """The engine's request, capture, log and temporary-path types are
    instances of aye's ``pytest`` types (abstract base classes), for
    annotations and ``isinstance``."""
    for shim_name, engine_name in (
        ("FixtureRequest", "Request"),
        ("CaptureFixture", "CaptureFixture"),
        ("LogCaptureFixture", "LogCaptureFixture"),
        ("TempPathFactory", "TempPathFactory"),
    ):
        base = getattr(pytest, shim_name, None)
        cls = getattr(aye_native_engine, engine_name, None)
        if isinstance(cls, type) and hasattr(base, "register"):
            base.register(cls)


_register_engine_types()


def _builtins(config: Any) -> dict[str, Any]:
    """The engine's builtin resources (``tmp_path``, ``monkeypatch``,
    ``capsys``…), made with aye's ``pytest`` types."""
    make = getattr(aye_native_engine, "builtin_resources", None)
    if make is None:
        return {}
    return dict(
        make(
            config,
            monkeypatch_class=pytest.MonkeyPatch,
            warnings_recorder_class=pytest.WarningsRecorder,
        )
    )


def assertrepr_compare(verbose: int) -> Callable[[str, Any, Any], list[str] | None]:
    """pytest's ``assertrepr_compare`` summary, for ``aye_rewrite``'s
    ``explain_comparison``: when the engine has more to say about a failed
    comparison than ``left op right``, the message's line shows each side
    cut to fit pytest's 80 columns (whole from ``-vv``), as pytest's own
    message does; the explanation itself is the failure's
    ``assertion.diff``."""

    def summary(op: str, left: Any, right: Any) -> list[str] | None:
        if not aye_native_engine.explain(op, left, right, verbose):
            return None
        use_ascii = (
            isinstance(left, str)
            and isinstance(right, str)
            and normalize("NFD", left) == normalize("NFD", right)
        )
        if verbose > 1:
            shown = [_unlimited(side, use_ascii) for side in (left, right)]
        else:
            # pytest's "15 chars indentation, 1 space around op".
            size = (80 - 15 - len(op) - 2) // 2
            shown = [aye_native_engine.saferepr(side, size, use_ascii) for side in (left, right)]
        return [f"{shown[0]} {op} {shown[1]}"]

    return summary


def _unlimited(value: Any, use_ascii: bool) -> str:
    """pytest's ``saferepr_unlimited``."""
    try:
        return ascii(value) if use_ascii else repr(value)
    except Exception:  # noqa: BLE001 - a broken __repr__ is the user's
        return aye_native_engine.saferepr(value, None, use_ascii)


@contextmanager
def _pytest_version() -> Iterator[None]:
    before = os.environ.get("PYTEST_VERSION")
    os.environ["PYTEST_VERSION"] = pytest.__version__
    try:
        yield
    finally:
        if before is None:
            os.environ.pop("PYTEST_VERSION", None)
        else:
            os.environ["PYTEST_VERSION"] = before


class NativeExecutor(aye_engine.Executor):
    """Refines and runs pytest suites with aye's front-end and engine: one
    session per worker (``_Session``), listed once, run batch after batch,
    ended by the last (``Batch.ends_session``) or by ``close``."""

    adapter = "pytest"

    def __init__(self, root: str, services: aye_engine.Services) -> None:
        super().__init__(root, services)
        self._session: _Session | None = None

    def frontend(self, context: Any) -> aye_pytest_frontend.Frontend:
        """A front-end for a session, with assertion rewriting installed for
        the modules it decides."""
        frontend = aye_pytest_frontend.Frontend(
            self.root, context if isinstance(context, dict) else {}, builtins=_builtins
        )
        aye_rewrite.install(
            frontend.should_rewrite,
            modules=frontend.rewrite_modules,
            warning=pytest.PytestAssertRewriteWarning,
        )
        return frontend

    def session(self, context: Any) -> _Session:
        """The worker's session for ``context``: the one listed so far, or
        a new one when the context differs in more than what only a batch
        carries (the old one ends first)."""
        context = context if isinstance(context, dict) else {}
        key = _session_key(context)
        session = self._session
        if session is not None and session.key != key:
            self.close()
            session = None
        if session is None:
            session = self._session = _Session(key, self.frontend(context))
        return session

    def refine(
        self, files: list[str], context: Any, *, digests_only: bool = False
    ) -> dict[str, Any]:
        """List ``files`` into the worker's session and return their tests
        as refined cases, in pytest's order, the diagnostics, and per file
        the digest of its ids (``digests``). With ``digests_only``, the
        digests alone: the cases stay listed here for the batches."""
        with _pytest_version():
            session = self.session(context)
            cases = session.list(files)
            diagnostics = session.diagnostics_for(files)
        with aye_pytest_frontend.no_automatic_gc():
            digests = _digests(files, cases)
            if digests_only:
                return {"cases": [], "diagnostics": diagnostics, "digests": digests}
            wire = session.frontend.case_dicts(cases)
        return {"cases": wire, "diagnostics": diagnostics, "digests": digests}

    def run(self, batch: aye_engine.Batch, sink: aye_engine.Sink) -> int:
        """Run ``batch`` on the worker's session: list its files not listed
        yet, keep its ids, run the cases on aye's engine, and end the
        session when the batch says so (``_end``). Returns pytest's
        status."""
        ends = batch.ends_session
        if not batch.ids:
            return self._end(sink, OK) if ends else NO_TESTS_COLLECTED
        context = batch.context if isinstance(batch.context, dict) else {}
        requested = list(dict.fromkeys(batch.ids))
        with _pytest_version():
            session = self.session(context)
            frontend = session.frontend
            files = list(dict.fromkeys(_file_of(test_id) for test_id in requested))
            listed = session.list(files)
            diagnostics = session.diagnostics_for(files)
            selection = Selection(requested, context)
            for diagnostic in diagnostics:
                if diagnostic.get("code") not in REFINEMENT_WARNINGS:
                    self._diagnostic(sink, diagnostic, selection)
            made = {case.id for case in listed}
            exact = selection.exact(made)
            if exact:
                # Every requested id is a test listing made: only those (and
                # the parameters of an unparametrized one) can be selected.
                candidates = session.claimed(requested)
            else:
                candidates = [case for case in listed if not session.deselected(case)]
            cases = selection.select(candidates, made, exact=exact)
            selection.report_missing(frontend, sink)
            self._duplicates(cases, sink)
            status = self._engine_run(session, cases, batch, sink, context, keep=not ends)
            failed = [p for p in frontend.failed_paths if any(_under(f, p) for f in files)]
            final = self.status(status, cases, failed, diagnostics)
            session.statuses.append(final)
        return self._end(sink, final) if ends else final

    def _end(self, sink: aye_engine.Sink, status: int) -> int:
        """End the worker's session in a batch whose status is ``status``,
        reporting into ``sink``: the lifetimes still open, with no test to
        blame (a ``lifetime_finished`` each, which fails the batch), then
        ``pytest_sessionfinish`` with the session's status (the worst of
        its batches'), ``pytest_unconfigure`` and the cleanups, whose
        failures are diagnostics. A ``session.exitstatus`` that
        ``pytest_sessionfinish`` set to 1 (a conftest's, pytest-cov's
        ``--cov-fail-under``) makes the batch's status 1, with a
        ``session-failed`` warning when nothing else explains it, as pytest
        then exits 1."""
        session, self._session = self._session, None
        if session is None:
            return status
        with _pytest_version():
            with session.frontend.logging_plugin():  # the test loop's end
                failures = session.engine.close(sink) if session.engine is not None else []
            exitstatus = session.exit_status()
            found = session.frontend.finish(exitstatus)
            after = session.frontend.session.exitstatus
        # pytest's configuration takes its import hook out when it ends.
        aye_rewrite.uninstall()
        session.forget()
        for diagnostic in found:
            sink.diagnostic(
                diagnostic["severity"],
                diagnostic["code"],
                diagnostic["path"],
                diagnostic["message"],
            )
        if any(d["severity"] == "error" and d["code"] == "internal-error" for d in found):
            return INTERNAL_ERROR
        if failures and status in (OK, NO_TESTS_COLLECTED):
            status = TESTS_FAILED
        if (
            isinstance(after, int)
            and int(after) == TESTS_FAILED
            and int(after) != exitstatus
            and status in (OK, NO_TESTS_COLLECTED)
        ):
            sink.diagnostic(
                "warning",
                "session-failed",
                "",
                "pytest_sessionfinish set the session's exit status to 1 (tests failed), but "
                "no test failed: a conftest or plugin failed the session (session.exitstatus, "
                "pytest-cov's --cov-fail-under)",
            )
            status = TESTS_FAILED
        return status

    def close(self) -> None:
        """End the worker's session: the lifetimes still open (the
        session's, the packages'), ``pytest_sessionfinish``,
        ``pytest_unconfigure``, the cleanups. With no batch to report into,
        what fails is written to stderr."""
        session, self._session = self._session, None
        if session is None:
            return
        with _pytest_version():
            with session.frontend.logging_plugin():  # the test loop's end
                failures = session.engine.close() if session.engine is not None else []
            found = session.frontend.finish(session.exit_status())
        # pytest's configuration takes its import hook out when it ends.
        aye_rewrite.uninstall()
        session.forget()
        for failure in failures:
            _stderr(f"aye: tearing down the session failed: {failure.message}")
        for diagnostic in found:
            _stderr(f"aye: {diagnostic['message']}")

    def _engine_run(
        self,
        session: _Session,
        cases: list[Any],
        batch: aye_engine.Batch,
        sink: aye_engine.Sink,
        context: dict[str, Any],
        *,
        keep: bool = True,
    ) -> int:
        """Run ``cases`` on the session's engine; ``keep`` keeps the
        session's lifetimes open for the next batch."""
        if not cases:
            return OK
        framework_args = without_junitxml(batch.policy.framework_args)
        rerun = aye_engine.rerun_args(context)
        engine = session.engine
        frontend = session.frontend
        if engine is None:
            engine = session.engine = aye_native_engine.Engine(
                self.paths,
                self.services,
                frontend.policy,
                frontend.hooks,
                frontend.builtins,
                config=frontend.config,
                current_test_env="PYTEST_CURRENT_TEST",
                return_warning=pytest.PytestReturnNotNoneWarning,
                disabled=frontend.disabled,
            )
            frontend.marks_changed = engine.tags_changed
            aye_rewrite.explain_comparison = assertrepr_compare(engine.verbose)
        engine.hooks = frontend.hooks
        # A conftest's flags stop a batch, not the session's next ones.
        frontend.session.shouldstop = frontend.session.shouldfail = False
        with frontend.logging_plugin():  # pytest's, around its test loop
            status = engine.run(
                cases,
                sink,
                maxfail=batch.policy.maxfail or 0,
                capture=batch.policy.capture,
                reproduce=lambda test_id: aye_engine.reproduce(test_id, framework_args, rerun),
                keep_session=keep,
            )
        return status if isinstance(status, int) else OK

    @staticmethod
    def status(
        engine_status: int,
        cases: Sequence[Any],
        failed_paths: Sequence[str],
        diagnostics: Sequence[dict[str, Any]],
    ) -> int:
        """pytest's status for the batch: the engine's when it has one, a
        usage error (4) or an internal error (3) the front-end reported,
        1 for a file that failed to collect, 5 when nothing ran."""
        codes = {d.get("code") for d in diagnostics if d.get("severity") == "error"}
        if engine_status not in (OK, TESTS_FAILED, NO_TESTS_COLLECTED):
            return engine_status
        if "internal-error" in codes:
            return INTERNAL_ERROR
        if "usage-error" in codes:
            return USAGE_ERROR
        if engine_status == TESTS_FAILED or failed_paths:
            return TESTS_FAILED
        if not cases:
            return NO_TESTS_COLLECTED
        return OK

    @staticmethod
    def _diagnostic(
        sink: aye_engine.Sink, diagnostic: dict[str, Any], selection: Selection
    ) -> None:
        if diagnostic.get("code") == "ignored":
            diagnostic = selection.ignored_warning(diagnostic)
        sink.diagnostic(
            diagnostic["severity"], diagnostic["code"], diagnostic["path"], diagnostic["message"]
        )

    @staticmethod
    def _duplicates(cases: Sequence[Any], sink: aye_engine.Sink) -> None:
        """Two tests with one id both run and report under it, as pytest
        runs both."""
        for test_id, count in Counter(case.id for case in cases).items():
            if count > 1:
                sink.diagnostic(
                    "warning",
                    "duplicate-id",
                    _file_of(test_id),
                    f"pytest runs more than one test with the id `{test_id}`; each one's "
                    "result is reported under that id, so it has more results than "
                    "collection lists",
                )


class _Session:
    """A worker's pytest session: the front-end, what it listed (in
    pytest's order), and the engine whose session and package lifetimes
    outlive each batch, as one pytest-xdist worker's session does."""

    def __init__(self, key: str, frontend: aye_pytest_frontend.Frontend) -> None:
        self.key = key
        self.frontend = frontend
        self.engine: aye_native_engine.Engine | None = None
        # Every case listed, those the hooks deselected included: by file,
        # and each one's place in the listing order.
        self.by_file: dict[str, list[Any]] = {}
        # By id, and by id without parameters (parametrized ones).
        self._by_id: dict[str, list[Any]] = {}
        self._by_base: dict[str, list[Any]] = {}
        self._position: dict[int, int] = {}
        self._deselected: set[int] = set()
        self._dropped = 0  # of the front-end's ``deselected``, those seen
        # The batches' statuses, for ``pytest_sessionfinish``.
        self.statuses: list[int] = []

    def list(self, files: Sequence[str]) -> list[Any]:
        """List the files not listed yet; the cases of ``files`` (and those
        listing them made elsewhere), in listing order."""
        new = self.frontend.extend(files)
        with aye_pytest_frontend.no_automatic_gc():
            return self._index(files, new)

    def _index(self, files: Sequence[str], new: list[Any]) -> list[Any]:
        """``list``'s part after the front-end listed: the new cases kept by
        file, id and place; those of ``files`` returned, in listing order."""
        dropped = self.frontend.deselected
        for item in dropped[self._dropped :]:
            case = getattr(item, "_case", None)
            if case is not None:
                self._deselected.add(id(case))
        self._dropped = len(dropped)
        position, by_file, by_id, by_base = self._position, self.by_file, self._by_id, self._by_base
        wanted = set(files)
        elsewhere = []  # cases listing ``files`` made in other files
        at = len(position)
        file = cases = None
        for case in new:
            test_id = case.id
            position[id(case)] = at
            at += 1
            path, sep, names = test_id.partition("::")
            if path != file:  # cases come file by file: one lookup each
                file = path
                cases = by_file.get(path)
                if cases is None:
                    cases = by_file[path] = []
            cases.append(case)  # type: ignore[union-attr]
            if path not in wanted:
                elsewhere.append(case)
            found = by_id.get(test_id)
            if found is None:
                by_id[test_id] = [case]
            else:
                found.append(case)
            if "[" in test_id:
                base = f"{path}{sep}{names.split('[', 1)[0]}" if sep else path
                by_base.setdefault(base, []).append(case)
        listed = [case for file in dict.fromkeys(files) for case in by_file.get(file, ())]
        listed += elsewhere
        listed.sort(key=lambda case: position[id(case)])
        return listed

    def forget(self) -> None:
        """Once the session has ended: the items let go of their cases and
        the front-end of the engine (the model and the engine are aye's
        alone), so that what they hold is freed as soon as nothing else
        holds it, never kept alive by the items or the configuration a
        conftest or a test still holds (or froze, ``gc.freeze``)."""
        for cases in self.by_file.values():
            for case in cases:
                item = case.extra.get("item")
                if getattr(item, "_case", None) is case:
                    item._case = None
        self.frontend.marks_changed = _nothing  # the engine's
        self.engine = None
        self.by_file.clear()
        self._by_id.clear()
        self._by_base.clear()
        self._position.clear()

    def exit_status(self) -> int:
        """The session's status for ``pytest_sessionfinish``: the worst
        batch's, 5 when none ran a test (or, without batches, nothing was
        listed)."""
        ran = [status for status in self.statuses if status != NO_TESTS_COLLECTED]
        if ran:
            return max(ran)
        if self.statuses or not self._position:
            return NO_TESTS_COLLECTED
        return OK

    def claimed(self, ids: Iterable[str]) -> list[Any]:
        """The listed cases ``ids`` name (each its own, and an id without
        parameters its parameters), those the hooks dropped left out, in
        listing order."""
        found: dict[int, Any] = {}
        for test_id in ids:
            for case in (*self._by_id.get(test_id, ()), *self._by_base.get(test_id, ())):
                if id(case) not in self._deselected:
                    found[id(case)] = case
        return sorted(found.values(), key=lambda case: self._position[id(case)])

    def deselected(self, case: Any) -> bool:
        """Whether a ``pytest_collection_modifyitems`` hook dropped it."""
        return id(case) in self._deselected

    def diagnostics_for(self, files: Sequence[str]) -> list[dict[str, Any]]:
        """What listing reported that concerns ``files``: what came while
        they were collected, and what concerns the whole session (the
        configuration, a hook that failed)."""
        frontend = self.frontend
        per_file = frontend.file_diagnostics
        attributed = {id(d) for found in per_file.values() for d in found}
        found = [d for d in frontend.diagnostics if id(d) not in attributed]
        for file in dict.fromkeys(files):
            found += per_file.get(file, ())
        return found


def _nothing() -> None:
    pass


def _session_key(context: dict[str, Any]) -> str:
    """What makes two contexts one session: all but what only a batch
    carries (the ids requested on the command line, those run before a
    crash, a rerun's arguments)."""
    shared = {k: v for k, v in context.items() if k not in BATCH_KEYS}
    return json.dumps(shared, sort_keys=True, default=repr)


def _digests(files: Sequence[str], cases: Sequence[Any]) -> dict[str, str]:
    """Per file, the SHA-1 of its cases' ids in order, joined by newlines
    (``files`` first, with or without cases)."""
    ids: dict[str, list[str]] = {file: [] for file in files}
    for case in cases:
        ids.setdefault(_file_of(case.id), []).append(case.id)
    return {
        file: sha1(
            "\n".join(found).encode("utf-8", "surrogatepass"), usedforsecurity=False
        ).hexdigest()
        for file, found in ids.items()
    }


def _stderr(text: str) -> None:
    stream = sys.__stderr__
    if stream is not None:
        try:
            stream.write(text.rstrip("\n") + "\n")
            stream.flush()
        except (OSError, ValueError):
            pass


class Selection:
    """Which collected items a batch runs, in the batch's order (the pytest
    adapter's ``_select``, over the front-end's items)."""

    def __init__(self, requested: list[str], context: dict[str, Any]) -> None:
        self.requested = requested
        self.order = {test_id: index for index, test_id in enumerate(requested)}
        self.typed = set(_ids(context.get("requested_ids")))
        self.ran = set(_ids(context.get("ran_ids")))
        self.made: set[str] = set()
        self.substituted: set[str] = set()

    def exact(self, made: set[str]) -> bool:
        """Whether every requested id is one listing made (none a file, a
        class, or parameters gone) and none ran before a crash: then only
        the cases they name can be selected."""
        return not self.ran and all(test_id in made for test_id in self.requested)

    def select(self, cases: list[Any], made: Iterable[str], *, exact: bool = False) -> list[Any]:
        """The batch's cases among ``cases`` (listed, in listing order), in
        the batch's order; ``made`` is every id listing made for the
        batch's files (deselected ones included). When ``exact``, only a
        case's own id, or its id without parameters, can claim it."""
        self.made.update(made)
        order = self.order
        kept: list[tuple[int, int, Any]] = []
        spare: dict[str, list[tuple[int, Any]]] = {}
        matched: set[str] = set()
        for position, item in enumerate(cases):
            test_id = item.id
            if test_id in self.ran:
                continue
            if exact:
                claims = {test_id} if test_id in order else set()
                if "[" in test_id and (base := unparametrized(test_id)) in order:
                    claims.add(base)
            else:
                claims = {key for key in selectors(test_id) if key in order}
            if claims:
                kept.append((min(self.order[key] for key in claims), position, item))
                matched |= claims
            else:
                spare.setdefault(unparametrized(test_id), []).append((position, item))
        kept += self._stand_ins(matched, spare)
        kept.sort(key=lambda entry: (entry[0], entry[1]))
        return [item for _, _, item in kept]

    def _stand_ins(
        self, matched: set[str], spare: dict[str, list[tuple[int, Any]]]
    ) -> list[tuple[int, int, Any]]:
        """A requested parametrized id pytest no longer makes (its id comes
        from an address, a uuid, a set's order) stands for the parameters
        of its test no requested id claims, unless the user typed it."""
        ran_of = Counter(unparametrized(test_id) for test_id in self.ran)
        moved = Counter(unparametrized(t) for t in self.ran if t not in self.made)
        kept: list[tuple[int, int, Any]] = []
        stood_for: set[str] = set()
        for test_id, index in self.order.items():
            base = unparametrized(test_id)
            if test_id in matched or test_id == base or test_id in self.typed:
                continue
            stand_ins = spare.pop(base, None)
            if stand_ins and moved[base] >= len(stand_ins):
                stand_ins = None
            if stand_ins:
                kept += [(index, position, item) for position, item in stand_ins]
                stood_for.add(base)
            elif ran_of[base]:
                stood_for.add(base)
            if base in stood_for:
                self.substituted.add(test_id)
        return kept

    def report_missing(self, frontend: aye_pytest_frontend.Frontend, sink: aye_engine.Sink) -> None:
        """Requested ids no item answers: skipped with their module or
        directory, covered by a collection error or an ignore rule, or
        ``not-found``."""
        missing = [
            test_id
            for test_id in self.requested
            if test_id not in self.made
            and test_id not in self.substituted
            and test_id not in self.ran
        ]
        if not missing:
            return
        found: set[str] = set()
        for test_id in self.made:
            found |= selectors(test_id)
        for test_id in missing:
            if test_id in found:
                continue
            path = frontend.absolute(_file_of(test_id))
            if path in frontend.ignored or frontend._under_broken(path):
                continue
            skipped = [reason for where, reason in frontend.skipped_paths if _under(test_id, where)]
            if skipped:
                sink.test_started(test_id)
                result = aye_engine.Result(test_id)
                result.add(aye_engine.Outcome.SKIPPED)
                result.reason = skipped[0]
                sink.test_finished(result)
            elif not any(_under(test_id, where) for where in frontend.failed_paths):
                sink.diagnostic(
                    "error", "not-found", _file_of(test_id), f"{test_id}: pytest found no such test"
                )

    def ignored_warning(self, diagnostic: dict[str, Any]) -> dict[str, Any]:
        """A file pytest ignores, as a batch words it: the requested tests
        it doesn't run."""
        file = diagnostic["path"]
        ids = [test_id for test_id in self.requested if _file_of(test_id) == file]
        named = ", ".join(ids[:MAX_NAMED])
        if len(ids) > MAX_NAMED:
            named += f" and {len(ids) - MAX_NAMED} more"
        message = diagnostic["message"].replace(
            "so it collects no tests from this file",
            f"so it does not run the {len(ids)} requested test{'s' if len(ids) != 1 else ''} "
            f"in this file: {named}",
        )
        return {**diagnostic, "severity": "warning", "message": message}


def _ids(value: Any) -> list[str]:
    if not isinstance(value, list | tuple):
        return []
    return [item for item in value if isinstance(item, str)]


def _file_of(test_id: str) -> str:
    return test_id.partition("::")[0]


def _under(test_id: str, path: str) -> bool:
    file = _file_of(test_id)
    return path in ("", ".") or file == path or file.startswith(f"{path}/")


def unparametrized(test_id: str) -> str:
    """``test_id`` without its parameters, which selects them all."""
    path, sep, names = test_id.partition("::")
    return f"{path}{sep}{names.split('[', 1)[0]}" if sep else path


def selectors(test_id: str) -> set[str]:
    """Every id that selects ``test_id``: itself, its directories, its file,
    its enclosing classes, and itself without parameters."""
    path, sep, names = test_id.partition("::")
    keys = {test_id, path}
    parts = path.split("/")
    for end in range(1, len(parts)):
        keys.add("/".join(parts[:end]))
    if sep:
        parts = names.split("[", 1)[0].split("::")
        for end in range(1, len(parts) + 1):
            keys.add(f"{path}::{'::'.join(parts[:end])}")
    return keys


def without_junitxml(args: Sequence[str]) -> list[str]:
    """``args`` without ``--junitxml``/``--junit-xml`` and their value: a
    rerun of one test would overwrite the run's report."""
    kept: list[str] = []
    rest = iter(args)
    for arg in rest:
        if arg in JUNITXML:
            next(rest, None)
            continue
        option, equals, _ = arg.partition("=")
        if equals and option in JUNITXML:
            continue
        kept.append(arg)
    return kept
