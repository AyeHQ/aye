"""aye's worker: the Python process the runner starts and talks to.

The runner embeds this file in its binary and runs it with ``python -c`` in
the project's interpreter, in the directory aye was started in. The worker
knows nothing about any framework: aye's engine and each adapter's executor
arrive as source in the handshake, and the worker installs them, builds the
executors and relays requests to them.

Protocol (``aye_protocol::Request`` and ``aye_protocol::Reply``), one JSON object
per line:

- stdin: ``hello`` first, then ``refine``, ``run`` or ``shutdown``, which (as
  EOF does) calls each executor's optional ``close()``, then ends the worker.
- stdout: replies (``ready``, ``refined``, ``batch_done``, ``failed``) and the
  executors' event records. Every line is JSON that a strict parser (serde)
  reads, whatever an executor sends (see ``encode``).
- ``refine`` with ``digests_only`` answers no case, and passes
  ``digests_only=True`` to an executor's ``refine`` that takes it. Answers
  for an executor that ``lists_session``, or to ``digests_only``, have
  ``digests``: the executor's, else per file the SHA-1 of its ids in order.

Before anything else, the worker keeps private copies of fds 0 and 1 for the
protocol, points fd 1 at stderr and fd 0 at the null device, so that a stray
print or ``input()`` can neither corrupt nor consume the stream. The copies are
close-on-exec and sit high (from half the descriptor limit, at most 512), out
of the way of tests that juggle low descriptors; a process forked from the
worker has them pointed at the null device and its messages dropped, so that
it can neither hold the stream open nor add to it. It leaves ``sys.path`` as
``python -c`` sets it, except that the leading ``''`` becomes the absolute
working directory, as under ``python -m pytest``.

What the protocol's path calls (``os.getpid``, the JSON encoder's own
functions rather than ``json.encoder``'s names for them, the decoder,
``traceback.format_exc``, the stream's ``write``) is bound when the worker
starts, so a test that replaces one (``mock.patch("os.getpid")``,
``json.encoder.encode_basestring``, and never puts it back) can neither
silence nor garble the stream.

It imports only the standard library, runs on Python 3.10 and later, and stays
small: Windows limits the length of the command line that carries it.
"""

import hashlib
import json
import linecache
import math
import os
import re
import signal
import sys
import threading
import traceback
import types

try:
    import fcntl
    import resource
except ImportError:  # Windows
    fcntl = resource = None

PROTOCOL = 3

# A ``failed`` message keeps this many characters, from the end.
MAX_MESSAGE = 8000

# An integer beyond a double's range, all that a strict parser (serde) holds
# past 64 bits, takes this many digits or more. Found in a line's UTF-8 with
# its digits kept and the rest made spaces.
_LONG = b"0" * 309
_DIGITS = bytes(ord("0" if chr(byte) in "0123456789" else " ") for byte in range(256))
_SURROGATES = re.compile("[\ud800-\udfff]")

# Bound now, before any test can replace them (see the module's docstring).
# ``JSONEncoder.encode`` looks ``encode_basestring`` and ``c_make_encoder`` up
# in ``json.encoder`` at every call, so the encoder is built from the C
# functions themselves; the pure-Python one only where there are none.
_getpid = os.getpid
_make_encoder, _quote, _str = json.encoder.c_make_encoder, json.encoder.encode_basestring, str
_py_encode = json.JSONEncoder(
    ensure_ascii=False, separators=(",", ":"), default=str, allow_nan=False
).encode
_decode = json.JSONDecoder().decode
_isfinite, _sha1 = math.isfinite, hashlib.sha1
_FLOAT_MAX = sys.float_info.max
_format_exc = traceback.format_exc
_open, _dup2, _close, _DEVNULL = os.open, os.dup2, os.close, os.devnull
_killpg, _getpgrp, _exit = getattr(os, "killpg", None), getattr(os, "getpgrp", None), os._exit


def _encode(message):
    """``message`` as compact JSON, non-ASCII kept, ``str`` of what JSON
    can't hold; NaN and infinities raise ``ValueError``."""
    if _make_encoder is None:
        return _py_encode(message)
    return "".join(_make_encoder({}, _str, _quote, None, ":", ",", False, False, False)(message, 0))


class Channel:
    """Writes protocol messages: one JSON object per line, flushed. Only the
    process that made it writes: a forked child's messages are dropped."""

    def __init__(self, stream):
        self._stream = stream
        self._write, self._flush = stream.write, stream.flush
        self._lock = threading.Lock()
        self.owner = _getpid()

    def send(self, message):
        if _getpid() != self.owner:
            return
        line = encode(message)
        with self._lock:
            try:
                self._write(line + "\n")
                self._flush()
            except BrokenPipeError:
                gone()


def gone():
    """The runner is gone (killed outright): nobody reads what the tests do
    anymore. End the worker and, when it leads its process group as the
    runner starts it, what its tests started, as the runner would have."""
    if _killpg is not None and _getpgrp() == _getpid():
        _killpg(_getpid(), signal.SIGKILL)
    _exit(1)


def encode(message):
    """``message`` as one line of JSON that a strict parser (serde) reads:
    NaN, infinities and integers beyond a double's range become ``null``,
    keys JSON can't hold become text, and lone surrogates (undecodable file
    names), which UTF-8 can't hold, become U+FFFD. What still can't be
    written (a cycle) raises: no invalid line is ever written."""
    try:
        line = _encode(message)
        if len(line) >= len(_LONG) and _beyond(line):
            raise ValueError("an integer may be beyond a double's range")
    except (ValueError, TypeError):
        line = _encode(_plain(message))
    if not line.isascii():
        try:
            line.encode("utf-8")
        except UnicodeEncodeError:
            line = _SURROGATES.sub("\ufffd", line)
    return line


def _beyond(line):
    """Whether ``line`` has 309 digits or more where a value starts, out of
    text: an integer that may be beyond a double's range. Quotes are counted
    once, from the last candidate (after ``:,[``: no backslash run cut)."""
    raw = line.encode("utf-8", "surrogatepass")
    digits = raw.translate(_DIGITS)
    at = digits.find(_LONG)
    counted = quotes = 0  # unescaped quotes in raw[:counted]
    while at >= 0:
        start = at - (raw[at - 1 : at] == b"-")
        if raw[start - 1 : start] in b":,[":  # b"" at the start, too
            text = raw[counted:start].replace(b"\\\\", b"")
            quotes += text.count(b'"') - text.count(b'\\"')
            counted = start
            if quotes % 2 == 0:
                return True
        end = digits.find(b" ", at)
        at = -1 if end < 0 else digits.find(_LONG, end)
    return False


def _plain(value):
    """``value`` with NaN, infinities and integers beyond a double's range as
    ``None``, and keys other than text, numbers, booleans and ``None`` (or
    numbers that become ``None``) as text."""
    if isinstance(value, float):
        return value if _isfinite(value) else None
    if isinstance(value, int) and not isinstance(value, bool):
        return value if abs(value) <= _FLOAT_MAX else None
    if isinstance(value, dict):
        return {_key(k): _plain(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_plain(v) for v in value]
    return value


def _key(key):
    """``key`` where JSON writes it as a key itself, else as text."""
    if isinstance(key, (str, bool)) or key is None:
        return key
    if isinstance(key, (int, float)) and _plain(key) is not None:
        return key
    return repr(key) if isinstance(key, float) else str(key)


def install(name, source):
    """Install ``source`` as the module ``name``, from memory. Its frames read
    ``<aye:name>`` and tracebacks show its lines."""
    filename = f"<aye:{name}>"
    module = types.ModuleType(name)
    sys.modules[name] = module
    linecache.cache[filename] = (len(source), None, source.splitlines(True), filename)
    try:
        code = compile(source, filename, "exec", dont_inherit=True)
        exec(code, module.__dict__)  # noqa: S102 - the runner's own embedded source
    except BaseException:
        del sys.modules[name]
        raise
    return module


class Worker:
    """Serves requests until ``shutdown`` or the end of input."""

    def __init__(self, channel):
        self.channel = channel
        self.engine = None
        self.executors = {}
        self.listing = set()  # adapters whose executor lists the session

    def serve(self, lines):
        """Handle each request line. Returns the exit status."""
        for raw in lines:
            if _getpid() != self.channel.owner:
                return 0  # a forked child that carried on: requests aren't its
            text = raw.decode("utf-8", "replace") if isinstance(raw, bytes) else raw
            if not text.strip():
                continue
            try:
                request = _decode(text)
                kind = request["type"]
            except (ValueError, KeyError, TypeError) as exc:
                self.failed(f"unreadable request ({exc}): {text[:200]}")
                continue
            if kind == "shutdown":
                break
            if kind == "hello":
                if not self.hello(request):
                    return 1
            elif self.engine is None:
                self.failed(f"{kind!r} request before hello")
            elif kind == "run":
                self.run(request)
            elif kind == "refine":
                self.refine(request)
            else:
                self.failed(f"unknown request type {kind!r}")
        self.close()
        return 0

    def close(self):
        """Call each executor's optional ``close()``."""
        for executor in self.executors.values():
            try:
                getattr(executor, "close", lambda: None)()
            except (Exception, SystemExit):  # noqa: BLE001 - the worker is ending
                sys.__stderr__.write(_format_exc())

    def hello(self, request):
        """Install the modules, build the services and the executors."""
        if request.get("protocol") != PROTOCOL:
            # Answer with this worker's version; the runner explains.
            self.ready()
            return True
        try:
            for module in request.get("modules", []):
                install(module["name"], module["source"])
            engine = sys.modules["aye_engine"]
            defaults = engine.default_services()
            services = engine.Services(
                {
                    name: defaults.get(name)
                    for name in request.get("services", [])
                    if defaults.has(name)
                }
            )
            executors = {
                spec["adapter"]: engine.load_entry(spec["entry"])(request["root"], services)
                for spec in request.get("executors", [])
            }
        except (Exception, SystemExit):  # noqa: BLE001 - reported to the runner
            self.failed(_format_exc())
            return False
        self.engine = engine
        self.executors = executors
        self.listing = {
            s["adapter"] for s in request.get("executors", []) if s.get("lists_session")
        }
        self.ready()
        return True

    def run(self, request):
        try:
            batch = self.engine.Batch(request)
            executor = self.executor(batch.adapter)
            status = executor.run(batch, self.engine.Sink(self.channel.send, batch=batch.id))
        except (Exception, SystemExit):  # noqa: BLE001 - reported; the worker goes on
            self.failed(_format_exc())
            return
        # The framework's own status, when it is one the runner can hold.
        if isinstance(status, bool) or not isinstance(status, int) or abs(status) >= 2**31:
            status = None
        else:
            status = int(status)
        self.channel.send({"reply": "batch_done", "batch": batch.id, "status": status})

    def refine(self, request):
        try:
            executor = self.executor(request["adapter"])
            files, context = list(request.get("files", [])), request.get("context")
            only = bool(request.get("digests_only"))
            code = getattr(executor.refine, "__code__", None)
            takes = code and code.co_varnames[: code.co_argcount + code.co_kwonlyargcount]
            asks = only and "digests_only" in (takes or ())
            refined = executor.refine(files, context, **({"digests_only": True} if asks else {}))
            cases = list(refined.get("cases", []))
            reply = {
                "reply": "refined",
                "cases": [] if only else cases,
                "diagnostics": list(refined.get("diagnostics", [])),
            }
            digests = refined.get("digests")
            if digests is None and (only or request["adapter"] in self.listing):
                digests = {file: [] for file in files}
                for case in cases:
                    digests.setdefault(case["id"].split("::")[0], []).append(case["id"])
                for file, ids in digests.items():
                    digests[file] = _sha1(
                        "\n".join(ids).encode("utf-8", "surrogatepass")
                    ).hexdigest()
            if digests is not None:
                reply["digests"] = dict(digests)
        except (Exception, SystemExit):  # noqa: BLE001 - reported; the worker goes on
            self.failed(_format_exc())
            return
        self.channel.send(reply)

    def executor(self, adapter):
        try:
            return self.executors[adapter]
        except KeyError:
            raise LookupError(f"this worker has no executor for the {adapter!r} adapter") from None

    def ready(self):
        self.channel.send(
            {
                "reply": "ready",
                "protocol": PROTOCOL,
                "pid": _getpid(),
                "python": sys.executable,
                "version": sys.version.splitlines()[0],
            }
        )

    def failed(self, message):
        if len(message) > MAX_MESSAGE:
            message = "[… truncated]\n" + message[-MAX_MESSAGE:]
        self.channel.send({"reply": "failed", "message": message})


def private(fd):
    """A close-on-exec copy of ``fd``, high enough that tests juggling low
    descriptors (systemd's socket activation uses fd 3) don't hit it."""
    try:
        base = min(resource.getrlimit(resource.RLIMIT_NOFILE)[0], 1024) // 2
        return fcntl.fcntl(fd, fcntl.F_DUPFD_CLOEXEC, max(base, 3))
    except (AttributeError, OSError, ValueError):  # Windows, odd limits
        return os.dup(fd)


def detach(*fds):
    """In a forked child: point the protocol's descriptors at the null device,
    so the child neither holds the stream open nor writes to it."""
    null = _open(_DEVNULL, os.O_RDWR)
    for fd in fds:
        _dup2(null, fd, inheritable=False)
    _close(null)


def main():
    """Take the standard streams for the protocol, then serve."""
    out_fd, in_fd = private(1), private(0)
    protocol_out = os.fdopen(out_fd, "w", encoding="utf-8", errors="replace", newline="\n")
    protocol_in = os.fdopen(in_fd, "rb")
    if hasattr(os, "register_at_fork"):
        os.register_at_fork(after_in_child=lambda: detach(out_fd, in_fd))
    os.dup2(2, 1)
    null = os.open(os.devnull, os.O_RDONLY)
    os.dup2(null, 0)
    os.close(null)
    reconfigure = getattr(sys.stdout, "reconfigure", None)
    if reconfigure is not None:
        reconfigure(line_buffering=True)
    if sys.path and sys.path[0] == "":
        sys.path[0] = os.getcwd()
    return Worker(Channel(protocol_out)).serve(protocol_in)


if __name__ == "__main__":
    sys.exit(main())
