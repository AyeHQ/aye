import contextlib
import hashlib
import json
import linecache
import os
import random
import signal
import subprocess
import sys
import time
import traceback
from pathlib import Path

import aye_worker
import pytest

HERE = Path(__file__).resolve().parent
WORKER = HERE.parent / "aye_worker.py"
ENGINE = HERE.parents[2] / "aye-engine" / "python" / "aye_engine.py"

FAKE = """
import gc
import json
import os
import sys
import time
import traceback

import aye_engine as engine


class FakeExecutor(engine.EngineExecutor):
    adapter = "fake"

    def load(self, batch):
        if "raise" in batch.ids:
            raise RuntimeError("cannot load")
        return [engine.Test(id, self.body(id)) for id in batch.ids]

    def body(self, id):
        def run():
            if id == "print":
                print("stray print")
                sys.stdout.flush()
            if id == "input":
                try:
                    input()
                except EOFError:
                    raise engine.Skip("stdin is empty") from None
            if id == "path":
                raise engine.Skip(sys.path[0])
            if id == "fail":
                raise AssertionError("boom")
            if id == "clobber":
                # What a test of systemd socket activation does to fds 3 and 4.
                for fd in (3, 4):
                    os.dup2(os.open(os.devnull, os.O_RDWR), fd)
            if id == "where":
                [channel] = [o for o in gc.get_objects() if type(o).__name__ == "Channel"]
                fd = channel._stream.fileno()
                raise engine.Skip(f"{fd} {os.get_inheritable(fd)}")
            if id == "nap":
                time.sleep(1)
            if id == "leak":
                # What a test that patches and never puts back does.
                os.getpid = lambda: 1
                json.dumps = lambda *args, **kwargs: '{"stub": true}'
                json.loads = lambda *args, **kwargs: {"type": "shutdown"}
                traceback.format_exc = lambda *args, **kwargs: "stub"
                # What `JSONEncoder.encode` looks up at every call.
                shout = lambda s: '"' + s.upper() + '"'
                json.encoder.encode_basestring = json.encoder.encode_basestring_ascii = shout
                json.encoder.c_make_encoder = None
            if id == "fork":
                os.fork()  # both processes carry on with the batch
            if id == "fork_server":
                pid = os.fork()
                if pid == 0:
                    time.sleep(60)
                    os._exit(0)
                with open(os.path.join(os.environ["FAKE_DIR"], "server.pid"), "w") as f:
                    f.write(str(pid))

        return run

    def refine(self, files, context):
        return {"cases": [{"id": f"{f}::test_x", "adapter": "fake"} for f in files]}


class ListingExecutor(engine.Executor):
    # Lists the session: records its calls, and its closing.

    adapter = "listing"
    breaks_on_close = False

    def __init__(self, root, services):
        super().__init__(root, services)
        self.calls = []

    def refine(self, files, context, digests_only=False):
        self.calls.append((files, digests_only))
        ids = [f"{f}::test_{n}" for f in files if "empty" not in f for n in "ab"]
        refined = {"cases": [{"id": id, "adapter": "listing"} for id in ids]}
        if context == "own":
            refined["digests"] = {f: "own" for f in files}
        return refined

    def close(self):
        self.calls.append("closed")
        if self.breaks_on_close:
            raise RuntimeError("close broke")


class StatusExecutor(engine.Executor):
    adapter = "status"

    def run(self, batch, sink):
        statuses = {"true": True, "none": None, "text": "1", "seven": 7, "huge": 2**40}
        return statuses[batch.ids[0]]
"""


def hello(*, services=("context", "cleanup"), modules=None, protocol=aye_worker.PROTOCOL):
    if modules is None:
        modules = [
            {"name": "aye_engine", "source": ENGINE.read_text()},
            {"name": "fake_executor", "source": FAKE},
        ]
    return {
        "type": "hello",
        "protocol": protocol,
        "root": "/repo",
        "modules": modules,
        "executors": [
            {"adapter": "fake", "entry": "fake_executor:FakeExecutor"},
            {"adapter": "status", "entry": "fake_executor:StatusExecutor"},
            {
                "adapter": "listing",
                "entry": "fake_executor:ListingExecutor",
                "lists_session": True,
            },
        ],
        "services": list(services),
    }


def run(batch_id, *ids, adapter="fake"):
    return {"type": "run", "id": batch_id, "adapter": adapter, "ids": list(ids), "policy": {}}


class Recorder:
    def __init__(self):
        self.lines = []

    def write(self, text):
        self.lines.append(text)

    def flush(self):
        pass

    @property
    def messages(self):
        return [json.loads(line) for line in "".join(self.lines).splitlines()]


@pytest.fixture(autouse=True)
def isolated_modules():
    """The worker installs modules into ``sys.modules``; put it back."""
    before = dict(sys.modules)
    yield
    for name in set(sys.modules) - set(before):
        del sys.modules[name]
    sys.modules.update(before)


def serve(*requests):
    out = Recorder()
    worker = aye_worker.Worker(aye_worker.Channel(out))
    status = worker.serve(json.dumps(r) + "\n" for r in requests)
    return status, out.messages, worker


def replies(messages):
    return [m for m in messages if "reply" in m]


def test_install_runs_the_source_as_a_module_with_readable_frames():
    module = aye_worker.install("aye_test_mod", "def f():\n    raise ValueError('x')\n")
    assert sys.modules["aye_test_mod"] is module
    try:
        module.f()
    except ValueError as exc:
        frame = traceback.extract_tb(exc.__traceback__)[-1]
    assert frame.filename == "<aye:aye_test_mod>"
    assert frame.line == "raise ValueError('x')"
    assert linecache.getline("<aye:aye_test_mod>", 1) == "def f():\n"


def test_a_module_that_fails_is_not_left_installed():
    with pytest.raises(ZeroDivisionError):
        aye_worker.install("aye_test_broken", "1 / 0\n")
    assert "aye_test_broken" not in sys.modules


def test_install_does_not_inherit_the_workers_future_flags():
    module = aye_worker.install("aye_test_flags", "def f(x: int): pass\n")
    assert module.f.__annotations__ == {"x": int}


def test_hello_answers_ready_and_builds_the_executors():
    status, messages, worker = serve(hello(services=("context", "nope")))
    assert status == 0
    [ready] = messages
    assert ready["reply"] == "ready"
    assert ready["protocol"] == aye_worker.PROTOCOL
    assert ready["pid"] == os.getpid()
    assert ready["python"] == sys.executable
    assert ready["version"] == sys.version.splitlines()[0]
    executor = worker.executors["fake"]
    assert executor.root == os.path.abspath("/repo")
    assert executor.services.names() == ["context"]
    assert worker.executors["status"].services is executor.services


def test_a_hello_of_another_protocol_gets_this_workers_version_only():
    _, messages, worker = serve(hello(protocol=99, modules=[{"name": "x", "source": "1 / 0"}]))
    assert messages == [
        {
            "reply": "ready",
            "protocol": aye_worker.PROTOCOL,
            "pid": os.getpid(),
            "python": sys.executable,
            "version": sys.version.splitlines()[0],
        }
    ]
    assert worker.engine is None


def test_a_failed_hello_is_reported_and_ends_the_worker():
    status, messages, _ = serve(
        hello(modules=[{"name": "aye_engine", "source": "raise ImportError('nope')"}]),
        run(0, "pass"),
    )
    assert status == 1
    [failed] = messages
    assert failed["reply"] == "failed"
    assert "ImportError: nope" in failed["message"]


def test_a_batch_streams_records_then_batch_done():
    status, messages, _ = serve(hello(), run(4, "pass", "fail"), {"type": "shutdown"})
    assert status == 0
    records = [m for m in messages if "event" in m]
    assert [r["event"] for r in records] == [
        "test_started",
        "test_finished",
        "test_started",
        "test_finished",
    ]
    assert all(r["batch"] == 4 for r in records)
    assert [r["outcome"] for r in records if r["event"] == "test_finished"] == [
        "passed",
        "failed",
    ]
    assert replies(messages)[-1] == {"reply": "batch_done", "batch": 4, "status": 1}


@pytest.mark.parametrize(
    ("returned", "status"),
    [("seven", 7), ("true", None), ("none", None), ("text", None), ("huge", None)],
)
def test_batch_status_is_an_int_or_nothing(returned, status):
    _, messages, _ = serve(hello(), run(0, returned, adapter="status"))
    assert replies(messages)[-1] == {"reply": "batch_done", "batch": 0, "status": status}


def test_an_executor_that_raises_fails_the_request_and_the_worker_goes_on():
    _, messages, _ = serve(hello(), run(0, "raise"), run(1, "pass"))
    got = replies(messages)
    assert got[1]["reply"] == "failed"
    assert "RuntimeError: cannot load" in got[1]["message"]
    assert got[2] == {"reply": "batch_done", "batch": 1, "status": 0}


def test_refine_answers_refined():
    _, messages, _ = serve(
        hello(), {"type": "refine", "adapter": "fake", "files": ["a.py"], "context": None}
    )
    assert replies(messages)[-1] == {
        "reply": "refined",
        "cases": [{"id": "a.py::test_x", "adapter": "fake"}],
        "diagnostics": [],
    }


def sha1(*ids):
    return hashlib.sha1("\n".join(ids).encode()).hexdigest()


def refine(*files, context=None, digests_only=False):
    request = {"type": "refine", "adapter": "listing", "files": list(files), "context": context}
    if digests_only:
        request["digests_only"] = True
    return request


def test_an_executor_that_lists_the_session_answers_digests_by_file():
    _, messages, worker = serve(hello(), refine("a.py", "empty.py"))
    [reply] = replies(messages)[1:]
    assert [case["id"] for case in reply["cases"]] == ["a.py::test_a", "a.py::test_b"]
    assert reply["digests"] == {"a.py": sha1("a.py::test_a", "a.py::test_b"), "empty.py": sha1()}
    assert worker.executors["listing"].calls == [(["a.py", "empty.py"], False), "closed"]


def test_digests_only_asks_the_executor_so_and_answers_no_case():
    _, messages, worker = serve(hello(), refine("a.py", digests_only=True))
    [reply] = replies(messages)[1:]
    assert reply == {
        "reply": "refined",
        "cases": [],
        "diagnostics": [],
        "digests": {"a.py": sha1("a.py::test_a", "a.py::test_b")},
    }
    assert worker.executors["listing"].calls[0] == (["a.py"], True)


def test_digests_only_works_with_an_executor_that_cannot_be_asked_so():
    request = {**refine("a.py", digests_only=True), "adapter": "fake"}
    _, messages, _ = serve(hello(), request)
    [reply] = replies(messages)[1:]
    assert reply["cases"] == []
    assert reply["digests"] == {"a.py": sha1("a.py::test_x")}


def test_the_executors_own_digests_are_kept():
    _, messages, _ = serve(hello(), refine("a.py", context="own", digests_only=True))
    assert replies(messages)[1]["digests"] == {"a.py": "own"}


def test_executors_are_closed_at_shutdown_and_at_the_end_of_input():
    for requests in ([{"type": "shutdown"}, refine("never.py")], []):
        status, messages, worker = serve(hello(), *requests)
        assert status == 0
        assert worker.executors["listing"].calls == ["closed"]
        assert len(replies(messages)) == 1


def test_an_executor_whose_close_raises_ends_the_worker_all_the_same(capfd):
    worker = aye_worker.Worker(aye_worker.Channel(Recorder()))

    def lines():
        yield json.dumps(hello()) + "\n"
        worker.executors["listing"].breaks_on_close = True

    assert worker.serve(lines()) == 0
    assert worker.executors["listing"].calls == ["closed"]
    assert "RuntimeError: close broke" in capfd.readouterr().err


def test_requests_it_cannot_serve_are_failed():
    status, messages, _ = serve(
        run(0, "pass"),
        hello(),
        {"type": "dance"},
        run(1, "pass", adapter="nope"),
    )
    assert status == 0
    got = replies(messages)
    assert got[0] == {"reply": "failed", "message": "'run' request before hello"}
    assert got[2] == {"reply": "failed", "message": "unknown request type 'dance'"}
    assert "no executor for the 'nope' adapter" in got[3]["message"]


def test_unreadable_lines_are_failed_and_blank_ones_skipped():
    out = Recorder()
    worker = aye_worker.Worker(aye_worker.Channel(out))
    status = worker.serve([b"\n", b"not json\n", b'{"no": "type"}\n'])
    assert status == 0
    assert [m["reply"] for m in out.messages] == ["failed", "failed"]
    assert out.messages[0]["message"].startswith("unreadable request")


def test_long_failures_keep_their_end():
    out = Recorder()
    aye_worker.Worker(aye_worker.Channel(out)).failed("x" * 10000 + "the end")
    message = out.messages[0]["message"]
    assert message.startswith("[… truncated]\n")
    assert message.endswith("the end")
    assert len(message) < aye_worker.MAX_MESSAGE + 20


def test_the_channel_writes_one_line_per_message():
    out = Recorder()
    aye_worker.Channel(out).send({"text": "é\nline", "path": Path("/x")})
    # A path is written as the platform spells it (\\x on Windows).
    path = json.dumps(str(Path("/x")))
    assert out.lines == ['{"text":"é\\nline","path":' + path + "}\n"]


def strict_json(line):
    """``line`` parsed as a strict parser (serde) reads it: no NaN or
    infinities, no integer beyond a double's range, nothing but UTF-8 (lone
    surrogates neither raw nor escaped)."""

    def refuse(constant):
        raise ValueError(f"not JSON: {constant}")

    def integer(text):
        if abs(int(text)) > sys.float_info.max:
            raise ValueError(f"beyond a double's range: {text[:20]}…")
        return int(text)

    line.encode("utf-8")
    value = json.loads(line, parse_constant=refuse, parse_int=integer)
    json.dumps(value, ensure_ascii=False).encode("utf-8")
    return value


def test_the_channel_never_writes_nan_or_infinity():
    out = Recorder()
    nan, inf = float("nan"), float("inf")
    aye_worker.Channel(out).send({"t": inf, "n": [nan, 1.5, (-inf,)], "k": {"x": nan, 2.0: 1}})
    [line] = out.lines
    assert strict_json(line) == {"t": None, "n": [None, 1.5, [None]], "k": {"x": None, "2.0": 1}}


def test_the_channel_writes_lone_surrogates_as_valid_utf8():
    out = Recorder()
    name = os.fsdecode(b"caf\xe9.txt") if os.name != "nt" else "caf\udce9.txt"
    channel = aye_worker.Channel(out)
    channel.send({"message": f"missing {name}", name: [name, "\ud83d"], "é": 1})
    channel.send({"path": name, "t": float("nan")})
    first, second = (strict_json(line) for line in out.lines)
    # As aye's engine cleans text: one U+FFFD for each.
    assert first == {
        "message": "missing caf\ufffd.txt",
        "caf\ufffd.txt": ["caf\ufffd.txt", "\ufffd"],
        "é": 1,
    }
    assert second == {"path": "caf\ufffd.txt", "t": None}


def test_the_channel_never_writes_an_integer_beyond_a_double():
    out = Recorder()
    channel = aye_worker.Channel(out)
    big, fits = 10**400, 10**308
    channel.send({"big": big, "neg": [-big], "fits": fits, "u64": 2**64, "text": "9" * 400})
    # Too long for Python to write at all.
    channel.send({"huge": 10**5000, "n": 1})
    first, second = (strict_json(line) for line in out.lines)
    assert first == {"big": None, "neg": [None], "fits": fits, "u64": 2**64, "text": "9" * 400}
    assert second == {"huge": None, "n": 1}


def test_long_runs_of_digits_in_text_are_written_as_they_are(monkeypatch):
    # packaging's ids (`test_x[>=1111…]`, `test_x[1111…]`) hold 309 digits
    # and more, in text only: written without the slow path.
    slow = []
    plain = aye_worker._plain
    monkeypatch.setattr(aye_worker, "_plain", lambda value: slow.append(value) or plain(value))
    out = Recorder()
    channel = aye_worker.Channel(out)
    run = "1" * 400
    texts = [f"t.py::test_x[>={run}]", f"t.py::test_x[{run}]", f"a,{run}", f":-{run}"]
    texts += ['\\\\",[' + run, run]  # escaped quotes don't end the text
    channel.send({"ids": texts, "n": 2**64, run: 1})
    assert slow == []
    channel.send({"big": [-(10**400)], "ids": texts})  # a number: the slow path
    assert slow
    first, second = (strict_json(line) for line in out.lines)
    assert first == {"ids": texts, "n": 2**64, run: 1}
    assert second == {"big": [None], "ids": texts}


def _reference_beyond(line):
    """``_beyond`` as first written: the quotes before each candidate
    counted from the line's start."""
    raw = line.encode("utf-8", "surrogatepass")
    digits = raw.translate(aye_worker._DIGITS)
    at = digits.find(aye_worker._LONG)
    while at >= 0:
        start = at - (raw[at - 1 : at] == b"-")
        if raw[start - 1 : start] in b":,[":
            text = raw[:start].replace(b"\\\\", b"")
            if (text.count(b'"') - text.count(b'\\"')) % 2 == 0:
                return True
        end = digits.find(b" ", at)
        at = -1 if end < 0 else digits.find(aye_worker._LONG, end)
    return False


def test_the_long_integer_check_reads_each_line_once_and_answers_as_before():
    run = "7" * 320
    texts = [
        f"t.py::test_x[>={run}]",
        f"a,{run}",
        f":-{run}",
        f'\\",[{run}',
        f'\\\\",[{run}',
        "back\\slash",
        f"[{run}",
        run,
    ]
    numbers = [10**330, -(10**330), 2**64, 1]
    rng = random.Random(5)
    answers = []
    for _ in range(300):
        message = {
            "ids": rng.sample(texts, rng.randint(0, len(texts))),
            "pairs": [[rng.choice(texts), rng.choice(numbers)] for _ in range(3)],
        }
        if rng.random() < 0.3:
            message["n"] = rng.choice(numbers)
        line = aye_worker._encode(message)
        answers.append(aye_worker._beyond(line))
        assert answers[-1] == _reference_beyond(line), line[:300]
    assert True in answers and False in answers


def test_the_channel_writes_keys_json_cannot_hold_as_text():
    out = Recorder()
    keys = {(1, 2): "pair", float("nan"): "nan", 10**400: "big", True: "yes", None: "no", 3: 3}
    aye_worker.Channel(out).send({"k": keys})
    [line] = out.lines
    assert strict_json(line) == {
        "k": {
            "(1, 2)": "pair",
            "nan": "nan",
            str(10**400): "big",
            "true": "yes",
            "null": "no",
            "3": 3,
        }
    }


def test_a_message_that_cannot_be_written_raises_and_writes_nothing():
    out = Recorder()
    cycle = []
    cycle.append(cycle)
    with pytest.raises((ValueError, RecursionError)):
        aye_worker.Channel(out).send({"cycle": cycle})
    assert out.lines == []


def test_only_the_process_that_made_the_channel_writes_to_it():
    out = Recorder()
    channel = aye_worker.Channel(out)
    channel.owner = -1  # as seen from a forked child
    channel.send({"reply": "ready"})
    assert out.lines == []
    status = aye_worker.Worker(channel).serve([json.dumps(hello()) + "\n"])
    assert status == 0
    assert out.lines == []


def start(tmp_path, *requests, **options):
    """The worker as the runner starts it, fed ``requests``."""
    process = subprocess.Popen(
        [sys.executable, "-c", WORKER.read_text()],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        cwd=tmp_path,
        env={**os.environ, "FAKE_DIR": str(tmp_path)},
        **options,
    )
    process.stdin.write("".join(json.dumps(r) + "\n" for r in requests).encode())
    process.stdin.close()
    return process


def messages_of(process):
    return [json.loads(line) for line in process.stdout.read().decode().splitlines()]


@pytest.mark.skipif(os.name == "nt", reason="POSIX descriptors")
def test_the_protocol_streams_sit_high_and_are_not_inherited(tmp_path):
    process = start(tmp_path, hello(), run(0, "where", "clobber", "pass"), {"type": "shutdown"})
    messages = messages_of(process)
    assert process.wait(timeout=60) == 0
    finished = {m["id"]: m for m in messages if m.get("event") == "test_finished"}
    fd, inheritable = finished["where"]["reason"].split()
    assert int(fd) > 4
    assert inheritable == "False"
    assert finished["clobber"]["outcome"] == "passed"
    assert finished["pass"]["outcome"] == "passed"
    assert messages[-1] == {"reply": "batch_done", "batch": 0, "status": 0}


@pytest.mark.skipif(not hasattr(os, "fork"), reason="needs fork")
def test_a_forked_child_that_carries_on_adds_nothing(tmp_path):
    process = start(tmp_path, hello(), run(0, "fork", "pass", "fail"), {"type": "shutdown"})
    messages = messages_of(process)
    assert process.wait(timeout=60) == 0
    finished = [m["id"] for m in messages if m.get("event") == "test_finished"]
    assert finished == ["fork", "pass", "fail"]
    assert [m["reply"] for m in messages if "reply" in m] == ["ready", "batch_done"]


@pytest.mark.skipif(not hasattr(os, "fork"), reason="needs fork")
def test_a_forked_child_does_not_hold_the_protocol_stream_open(tmp_path):
    started = time.monotonic()
    process = start(
        tmp_path,
        hello(),
        run(0, "fork_server", "pass"),
        {"type": "shutdown"},
        stderr=subprocess.DEVNULL,
    )
    try:
        messages = messages_of(process)  # reads to the end of the stream
        assert time.monotonic() - started < 30
        assert process.wait(timeout=60) == 0
        assert messages[-1] == {"reply": "batch_done", "batch": 0, "status": 0}
    finally:
        server = tmp_path / "server.pid"
        if server.exists():
            with contextlib.suppress(ProcessLookupError):
                os.kill(int(server.read_text()), signal.SIGKILL)


@pytest.mark.skipif(os.name == "nt", reason="POSIX process groups")
def test_a_worker_whose_runner_is_gone_stops_at_its_next_message(tmp_path):
    process = subprocess.Popen(
        [sys.executable, "-c", WORKER.read_text()],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        cwd=tmp_path,
        start_new_session=True,  # a group leader, as the runner starts it
    )
    process.stdin.write((json.dumps(hello()) + "\n").encode())
    process.stdin.write((json.dumps(run(0, "nap", *["nap"] * 30)) + "\n").encode())
    process.stdin.flush()
    assert json.loads(process.stdout.readline())["reply"] == "ready"
    assert json.loads(process.stdout.readline())["event"] == "test_started"
    process.stdout.close()  # what the runner's death does to the stream
    started = time.monotonic()
    assert process.wait(timeout=20) == -signal.SIGKILL
    assert time.monotonic() - started < 5
    process.stdin.close()


def test_the_process_keeps_its_streams_for_the_protocol(tmp_path):
    requests = "".join(
        json.dumps(r) + "\n"
        for r in (hello(), run(0, "print", "input", "path"), {"type": "shutdown"})
    )
    done = subprocess.run(
        [sys.executable, "-c", WORKER.read_text()],
        input=requests.encode(),
        capture_output=True,
        cwd=tmp_path,
        timeout=60,
        check=False,
    )
    assert done.returncode == 0, done.stderr.decode()
    lines = done.stdout.decode().splitlines()
    messages = [json.loads(line) for line in lines]
    finished = {m["id"]: m for m in messages if m.get("event") == "test_finished"}
    assert finished["print"]["outcome"] == "passed"
    assert finished["input"]["reason"] == "stdin is empty"
    assert finished["path"]["reason"] == str(tmp_path)
    assert "stray print" in done.stderr.decode()
    assert messages[-1] == {"reply": "batch_done", "batch": 0, "status": 0}


def test_what_tests_replace_doesnt_change_what_the_worker_writes(monkeypatch):
    requests = [
        json.dumps(r) + "\n"
        for r in (hello(), run(0, "pass", "fail"), run(1, "raise"), {"type": "shutdown"})
    ]
    out = Recorder()
    with monkeypatch.context() as patched:
        patched.setattr(os, "getpid", lambda: 1)
        patched.setattr(json, "dumps", lambda *args, **kwargs: '{"stub": true}')
        patched.setattr(json, "loads", lambda *args, **kwargs: {"type": "shutdown"})
        patched.setattr(traceback, "format_exc", lambda *args, **kwargs: "stub")
        shout = lambda s: '"' + s.upper() + '"'
        patched.setattr(json.encoder, "encode_basestring", shout)
        patched.setattr(json.encoder, "encode_basestring_ascii", shout)
        patched.setattr(json.encoder, "c_make_encoder", None)
        patched.setattr(json.JSONEncoder, "encode", lambda self, o: '{"stub": true}')
        status = aye_worker.Worker(aye_worker.Channel(out)).serve(requests)
    assert status == 0
    messages = out.messages
    assert [m["reply"] for m in messages if "reply" in m] == ["ready", "batch_done", "failed"]
    assert messages[0]["pid"] == os.getpid()
    assert [m["outcome"] for m in messages if m.get("event") == "test_finished"] == [
        "passed",
        "failed",
    ]
    assert "RuntimeError: cannot load" in messages[-1]["message"]


def test_a_test_that_replaces_what_the_worker_uses_for_good_changes_nothing(tmp_path):
    # `os.getpid` once made the worker drop every message as a forked
    # child's, `batch_done` included (the run hung); `json.dumps` garbled them.
    process = start(
        tmp_path,
        hello(),
        run(0, "pass", "leak", "fail", "pass"),
        run(1, "raise"),
        run(2, "pass"),
        {"type": "shutdown"},
    )
    messages = messages_of(process)
    assert process.wait(timeout=60) == 0
    finished = [(m["id"], m["outcome"]) for m in messages if m.get("event") == "test_finished"]
    assert finished == [
        ("pass", "passed"),
        ("leak", "passed"),
        ("fail", "failed"),
        ("pass", "passed"),
        ("pass", "passed"),
    ]
    replies = [m for m in messages if "reply" in m]
    assert [m["reply"] for m in replies] == ["ready", "batch_done", "failed", "batch_done"]
    assert replies[1] == {"reply": "batch_done", "batch": 0, "status": 1}
    assert "RuntimeError: cannot load" in replies[2]["message"]
