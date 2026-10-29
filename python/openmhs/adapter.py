"""Write OpenMHS adapters in Python (docs/adapters/python-sdk.md).

An adapter runs as its own process. The gateway starts it from a permission manifest,
talks to it over stdin and stdout (spec/adapter-protocol.md), and keeps authorization,
the command lifecycle and the audit trail to itself. The adapter only translates between
OpenMHS and the device: it never decides whether a command is allowed, never retries a
state-changing command, and never reports success it has not observed.

Standard library only::

    from openmhs.adapter import Adapter, Applied, AdapterError, reading, run

    class Lamp(Adapter):
        adapter_id = "acme.lamp"
        version = "0.1.0"

        async def describe(self):
            return DESCRIPTOR

        async def read(self, capability):
            return reading(self.on)

        async def execute(self, command, progress):
            ...
            return Applied()

    if __name__ == "__main__":
        run(Lamp())

stdout belongs to the protocol. :func:`serve` moves it aside before anything else runs,
so ``print`` (and C libraries writing to file descriptor 1) end up on stderr, which the
gateway logs.
"""

from __future__ import annotations

import asyncio
import json
import os
import sys
import threading
import traceback
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any, Callable, Optional, Union
from urllib.parse import urlsplit

PROTOCOL_VERSIONS = ("0.1",)

#: Adapter error classes (spec/errors.md). Whether a failed command may still have changed
#: the device follows from the class; "timeout", "disconnected", "malformed_response" and
#: "internal" mean it may have.
ERROR_KINDS = (
    "timeout",
    "disconnected",
    "malformed_response",
    "device_failure",
    "not_sent",
    "unsupported",
    "internal",
)

#: Faults a conformance fixture can inject (spec/conformance.md §3).
FAULTS = ("link_down", "no_response", "lost_ack", "garbled", "refuse")

PARSE_ERROR = -32700
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
ADAPTER_ERROR = -32000
NOT_INITIALIZED = -32001
UNSUPPORTED_VERSION = -32002


class AdapterError(Exception):
    """A classified failure. ``kind`` is one of :data:`ERROR_KINDS`."""

    def __init__(self, kind: str, message: str):
        if kind not in ERROR_KINDS:
            raise ValueError(f"unknown adapter error kind {kind!r}; expected one of {ERROR_KINDS}")
        super().__init__(message)
        self.kind = kind
        self.message = message


@dataclass(frozen=True)
class Command:
    """A validated command. Values are already in the descriptor's units."""

    command_id: str
    capability: str
    operation: str  # "write" or "invoke"
    value: Any
    params: dict
    idempotent: bool
    deadline: str  # RFC 3339; the gateway stops waiting then


@dataclass(frozen=True)
class Applied:
    """The device reports the change applied. ``result`` holds an action's named results."""

    result: dict = field(default_factory=dict)


@dataclass(frozen=True)
class Rejected:
    """The device refused the command and applied nothing."""

    reason: str


def now() -> str:
    """The current time as an RFC 3339 string with milliseconds."""
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def quantity(value: float, unit: str) -> dict:
    return {"value": value, "unit": unit}


def reading(
    value: Any, *, timestamp: Optional[str] = None, source: str = "device", quality: str = "GOOD"
) -> dict:
    """A state value: what :meth:`Adapter.read` returns.

    ``timestamp`` should be when the device measured the value; it defaults to now.
    ``source`` is "device", "cache", "derived" or "simulator"; ``quality`` is "GOOD",
    "UNCERTAIN" or "BAD".
    """
    return {
        "value": value,
        "timestamp": timestamp or now(),
        "source": source,
        "quality": quality,
    }


def _endpoint(url: str) -> tuple:
    parts = urlsplit(url)
    return (parts.scheme.lower(), (parts.hostname or "").lower(), parts.port)


@dataclass(frozen=True)
class Permissions:
    """What the manifest declares the adapter may touch.

    The gateway does not enforce these from outside the process (spec/permission-manifest.md);
    calling the checks before opening a connection is what makes them hold.
    """

    network: tuple = ()
    serial: tuple = ()
    filesystem_read: tuple = ()
    filesystem_write: tuple = ()

    def check_network(self, url: str) -> None:
        """Raise ``not_sent`` unless ``url``'s scheme, host and port are declared."""
        target = _endpoint(url)
        if not any(_endpoint(e) == target for e in self.network):
            raise AdapterError(
                "not_sent", f"{url} is not among the manifest's endpoints.network {list(self.network)}"
            )

    def check_serial(self, port: str) -> None:
        if port not in self.serial:
            raise AdapterError(
                "not_sent", f"{port} is not among the manifest's endpoints.serial {list(self.serial)}"
            )

    def check_read(self, path: str) -> str:
        """Raise ``not_sent`` unless ``path`` is, or is inside, a declared ``filesystem.read``
        (or ``write``) entry. Relative paths are relative to the working directory, which is
        ``runtime.cwd`` or the manifest's directory. Returns the absolute path."""
        return _check_path(path, self.filesystem_read + self.filesystem_write, "read")


def _check_path(path: str, allowed: tuple, what: str) -> str:
    target = os.path.realpath(path)
    for entry in allowed:
        root = os.path.normcase(os.path.realpath(entry))
        try:
            if os.path.commonpath([os.path.normcase(target), root]) == root:
                return target
        except ValueError:  # different drives
            pass
    raise AdapterError(
        "not_sent", f"{path} is not among the manifest's filesystem.{what} {list(allowed)}"
    )


class Progress:
    """Passed to :meth:`Adapter.execute`."""

    def __init__(self, send: Callable[[str, dict], None], command_id: str):
        self._send = send
        self._command_id = command_id

    def accepted(self, external_ref: Optional[str] = None) -> None:
        """Report that the device acknowledged the command (lifecycle ACCEPTED).

        ``external_ref`` is an opaque identifier the device or its server gave the work (a
        LabThings invocation id, a job id): the gateway keeps it in the receipt
        (``adapter_result.external_ref``) so an unresolved command can be matched to the work it
        started. At most 256 characters, no control characters; otherwise it is dropped."""
        msg = {"command_id": self._command_id, "event": "accepted"}
        if external_ref:
            msg["external_ref"] = external_ref
        self._send("progress", msg)


HealthResult = Union[str, tuple]


class Adapter:
    """Base class. Override what the device supports; the defaults refuse."""

    #: Must equal the manifest's and the descriptor's ``adapter.id``.
    adapter_id: str = ""
    version: str = "0.0.0"
    #: Faults this adapter can inject when run by the conformance suite (:data:`FAULTS`).
    fixture_faults: tuple = ()
    #: Whether the adapter calls :meth:`report_deliveries`.
    fixture_counts_deliveries: bool = False

    config: dict
    permissions: Permissions
    _notify: Optional[Callable[[str, dict], None]] = None

    async def initialize(self, config: dict, permissions: Permissions) -> None:
        """Called once, before anything else. ``config`` is the manifest's ``config``."""
        self.config = config
        self.permissions = permissions

    async def describe(self) -> dict:
        """The device descriptor (spec/descriptor.md). Must not change between restarts."""
        raise AdapterError("unsupported", "describe is not implemented")

    async def connect(self) -> None:
        pass

    async def health(self) -> HealthResult:
        """"healthy", or ("degraded" | "disconnected", "why")."""
        return "healthy"

    async def read(self, capability: str) -> dict:
        """Return :func:`reading` for a readable capability."""
        raise AdapterError("unsupported", f"cannot read {capability!r}")

    async def execute(self, command: Command, progress: Progress) -> Union[Applied, Rejected]:
        """Carry out a write or invoke, once. Call ``progress.accepted()`` when the device
        acknowledges. Raise :class:`AdapterError` with the class that matches what is known:
        ``not_sent`` only if nothing reached the device."""
        raise AdapterError("unsupported", f"cannot execute {command.capability!r}")

    async def shutdown(self) -> None:
        pass

    async def fixture_inject(self, fault: str) -> None:
        """Start a conformance fault; only called for faults in :attr:`fixture_faults`."""
        raise AdapterError("unsupported", f"cannot inject {fault}")

    async def fixture_clear(self) -> None:
        pass

    def report_deliveries(self, count: int) -> None:
        """Conformance only: how many state-changing requests reached the device so far."""
        if self._notify is not None:
            self._notify("fixture.deliveries", {"count": count})


class _TooLong:
    pass


_TOO_LONG = _TooLong()


class _BadParams(Exception):
    pass


def _param(params: Any, name: str, kind: type) -> Any:
    if not isinstance(params, dict) or not isinstance(params.get(name), kind):
        raise _BadParams(f"params.{name} must be a {kind.__name__}")
    return params[name]


def _claim_stdout():
    """Keep the real stdout for the protocol and point file descriptor 1 at stderr."""
    sys.stdout.flush()
    fd = os.dup(sys.stdout.fileno())
    os.dup2(sys.stderr.fileno(), sys.stdout.fileno())
    sys.stdout = sys.stderr
    return os.fdopen(fd, "wb")


class _Server:
    def __init__(self, adapter: Adapter, out):
        self.adapter = adapter
        self.out = out
        self.max_bytes = 1 << 20
        self.initialized = False
        self.fixture = False
        self.tasks: set = set()
        adapter._notify = self.notify

    # Output. Everything is written from the event loop thread, one whole line at a time.

    def _write(self, msg: dict) -> None:
        line = json.dumps(msg, separators=(",", ":"), ensure_ascii=False, allow_nan=False).encode()
        if len(line) > self.max_bytes:
            raise ValueError(f"message of {len(line)} bytes exceeds the {self.max_bytes}-byte limit")
        self.out.write(line + b"\n")
        self.out.flush()

    def notify(self, method: str, params: dict) -> None:
        self._write({"jsonrpc": "2.0", "method": method, "params": params})

    def error(self, id: Any, code: int, message: str, data: Optional[dict] = None) -> None:
        err: dict = {"code": code, "message": message}
        if data is not None:
            err["data"] = data
        self._write({"jsonrpc": "2.0", "id": id, "error": err})

    def reply(self, id: Any, result: Any) -> None:
        try:
            self._write({"jsonrpc": "2.0", "id": id, "result": result})
        except (TypeError, ValueError) as e:
            # The handler returned something that is not JSON. If it was a command, it may
            # have been carried out, so the class is "internal" (outcome uncertain).
            self.error(id, ADAPTER_ERROR, f"unencodable result: {e}", {"kind": "adapter.internal"})

    # Input.

    def _read_stdin(self, loop: asyncio.AbstractEventLoop, queue: asyncio.Queue) -> None:
        stream = sys.stdin.buffer

        def put(item: Any) -> bool:
            try:
                loop.call_soon_threadsafe(queue.put_nowait, item)
                return True
            except RuntimeError:  # the loop has closed
                return False

        while True:
            limit = self.max_bytes
            line = stream.readline(limit + 2)
            if not line:
                put(None)
                return
            if not line.endswith(b"\n"):
                if len(line) < limit + 2:  # end of input inside a line
                    put(None)
                    return
                while line and not line.endswith(b"\n"):
                    line = stream.readline(1 << 16)
                if not put(_TOO_LONG):
                    return
                continue
            if not put(line.rstrip(b"\r\n")):
                return

    async def run(self) -> int:
        loop = asyncio.get_running_loop()
        queue: asyncio.Queue = asyncio.Queue()
        threading.Thread(target=self._read_stdin, args=(loop, queue), daemon=True).start()
        while True:
            line = await queue.get()
            if line is None:
                break  # the gateway has gone
            if line is _TOO_LONG:
                self.error(None, INVALID_REQUEST, "message exceeds the size limit")
                continue
            if not line.strip():
                continue
            try:
                msg = json.loads(line)
            except ValueError as e:
                self.error(None, PARSE_ERROR, f"not JSON: {e}")
                continue
            if not isinstance(msg, dict) or msg.get("jsonrpc") != "2.0":
                self.error(None, INVALID_REQUEST, "not a JSON-RPC 2.0 message")
                continue
            if "id" not in msg or "method" not in msg:
                # The host defines no notifications yet, and the SDK sends no requests
                # that would be answered.
                continue
            id = msg["id"]
            method = msg.get("method")
            params = msg.get("params")
            if method == "ping":
                self.reply(id, None)
            elif method == "initialize":
                await self._initialize(id, params)
            elif not self.initialized:
                self.error(id, NOT_INITIALIZED, f"{method} before initialize")
            elif method == "shutdown":
                await self._shutdown(id)
                return 0
            elif method in self.HANDLERS and (self.fixture or not method.startswith("fixture.")):
                task = asyncio.ensure_future(self._handle(id, self.HANDLERS[method], params))
                self.tasks.add(task)
                task.add_done_callback(self.tasks.discard)
            else:
                self.error(id, METHOD_NOT_FOUND, f"unknown method {method!r}")
        await self._close()
        return 0

    async def _handle(self, id: Any, handler, params: Any) -> None:
        try:
            result = await handler(self, params)
        except AdapterError as e:
            self.error(id, ADAPTER_ERROR, e.message, {"kind": f"adapter.{e.kind}"})
        except _BadParams as e:
            self.error(id, INVALID_PARAMS, str(e))
        except asyncio.CancelledError:
            raise
        except Exception as e:  # a bug in the adapter
            traceback.print_exc(file=sys.stderr)
            self.error(id, ADAPTER_ERROR, f"{type(e).__name__}: {e}", {"kind": "adapter.internal"})
        else:
            self.reply(id, result)

    async def _initialize(self, id: Any, params: Any) -> None:
        if self.initialized:
            self.error(id, INVALID_REQUEST, "already initialized")
            return
        offered = params.get("protocol_versions") if isinstance(params, dict) else None
        chosen = next((v for v in offered or [] if v in PROTOCOL_VERSIONS), None)
        if chosen is None:
            self.error(
                id,
                UNSUPPORTED_VERSION,
                f"none of the offered protocol versions {offered} is supported",
                {"supported": list(PROTOCOL_VERSIONS)},
            )
            return
        limits = params.get("limits") or {}
        if isinstance(limits.get("max_message_bytes"), int):
            self.max_bytes = limits["max_message_bytes"]
        perms = params.get("permissions") or {}
        fs = perms.get("filesystem") or {}
        permissions = Permissions(
            network=tuple(perms.get("network") or ()),
            serial=tuple(perms.get("serial") or ()),
            filesystem_read=tuple(fs.get("read") or ()),
            filesystem_write=tuple(fs.get("write") or ()),
        )
        self.fixture = bool(params.get("fixture"))
        try:
            await self.adapter.initialize(dict(params.get("config") or {}), permissions)
        except AdapterError as e:
            self.error(id, ADAPTER_ERROR, e.message, {"kind": f"adapter.{e.kind}"})
            return
        except Exception as e:
            traceback.print_exc(file=sys.stderr)
            self.error(id, ADAPTER_ERROR, f"{type(e).__name__}: {e}", {"kind": "adapter.internal"})
            return
        result: dict = {
            "protocol_version": chosen,
            "adapter": {"id": self.adapter.adapter_id, "version": self.adapter.version},
        }
        # An adapter that can inject no fault offers no fixture, and the conformance
        # suite then refuses to send it test commands.
        if self.fixture and self.adapter.fixture_faults:
            result["fixture"] = {
                "faults": [f for f in self.adapter.fixture_faults if f in FAULTS],
                "deliveries": self.adapter.fixture_counts_deliveries,
            }
        self.initialized = True
        self.reply(id, result)

    async def _shutdown(self, id: Any) -> None:
        try:
            await self.adapter.shutdown()
        except Exception:
            traceback.print_exc(file=sys.stderr)
        self.reply(id, None)
        await self._cancel_tasks()

    async def _close(self) -> None:
        await self._cancel_tasks()
        try:
            await self.adapter.shutdown()
        except Exception:
            traceback.print_exc(file=sys.stderr)

    async def _cancel_tasks(self) -> None:
        for task in list(self.tasks):
            task.cancel()
        if self.tasks:
            await asyncio.gather(*self.tasks, return_exceptions=True)

    # Methods.

    async def _describe(self, params: Any) -> dict:
        d = await self.adapter.describe()
        if not isinstance(d, dict):
            raise TypeError("describe() must return a dict")
        return d

    async def _connect(self, params: Any) -> None:
        await self.adapter.connect()

    async def _health(self, params: Any) -> dict:
        h = await self.adapter.health()
        status, message = (h, None) if isinstance(h, str) else (h[0], h[1])
        if status not in ("healthy", "degraded", "disconnected"):
            raise TypeError(f"health() returned unknown status {status!r}")
        return {"status": status} if message is None else {"status": status, "message": str(message)}

    async def _read(self, params: Any) -> dict:
        capability = _param(params, "capability", str)
        return await self.adapter.read(capability)

    async def _execute(self, params: Any) -> dict:
        c = _param(params, "command", dict)
        try:
            command = Command(
                command_id=c["command_id"],
                capability=c["capability"],
                operation=c["operation"],
                value=c.get("value"),
                params=c.get("params") or {},
                idempotent=bool(c["idempotent"]),
                deadline=c["deadline"],
            )
        except KeyError as e:
            raise _BadParams(f"params.command.{e.args[0]} is missing") from None
        report = await self.adapter.execute(command, Progress(self.notify, command.command_id))
        if isinstance(report, Applied):
            return {"status": "applied", "result": dict(report.result)}
        if isinstance(report, Rejected):
            return {"status": "rejected", "reason": report.reason}
        raise TypeError(f"execute() must return Applied or Rejected, not {type(report).__name__}")

    async def _fixture_inject(self, params: Any) -> None:
        fault = _param(params, "fault", str)
        if fault not in self.adapter.fixture_faults:
            raise AdapterError("unsupported", f"this fixture cannot inject {fault}")
        await self.adapter.fixture_inject(fault)

    async def _fixture_clear(self, params: Any) -> None:
        await self.adapter.fixture_clear()

    HANDLERS = {
        "describe": _describe,
        "connect": _connect,
        "health": _health,
        "read": _read,
        "execute": _execute,
        "fixture.inject": _fixture_inject,
        "fixture.clear": _fixture_clear,
    }


def serve(adapter: Adapter) -> int:
    """Speak the protocol on stdin/stdout until the gateway says ``shutdown`` or goes away."""
    out = _claim_stdout()
    return asyncio.run(_Server(adapter, out).run())


def run(adapter: Adapter) -> None:
    """:func:`serve`, then exit the process with its status."""
    sys.exit(serve(adapter))
