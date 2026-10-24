"""A read-only OctoPrint adapter (docs/adapters/octoprint.md).

OctoPrint is a web server that drives a 3D printer over a serial link. This adapter reads
the printer's state and temperatures through OctoPrint's documented HTTP API
(https://docs.octoprint.org/en/master/api/). It exists to exercise OpenMHS against a real
external service (OctoPrint's bundled Virtual Printer plugin needs no hardware), not to
operate a printer.

It is read-only by construction, not by convention:

* every request goes through ``OctoPrintClient.get``, which sends ``GET`` and nothing else,
  and only to the paths in ``GET_PATHS``;
* there is no code path that sends G-code, starts or cancels a job, sets a temperature,
  moves an axis or changes the connection;
* the descriptor may declare only ``read`` operations; a descriptor that declares more
  is refused at start-up.

The API key is a credential the manifest passes by name (``credentials``); the adapter
reads it from its environment, sends it in the ``X-Api-Key`` header and never logs it.
"""

from __future__ import annotations

import asyncio
import ipaddress
import json
import math
import os
import socket
import sys
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlencode, urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "python"))

from openmhs.adapter import Adapter, AdapterError, Permissions, reading, run  # noqa: E402

READ_ONLY = {"read", "subscribe"}

#: The only OctoPrint paths this adapter will ever request, always with GET.
GET_PATHS = frozenset({"/api/connection", "/api/printer"})

MAX_BODY = 1 << 20

#: Values of the ``printer.state`` capability.
STATES = ("offline", "connecting", "operational", "printing", "paused", "error")

#: Where each temperature capability lives in the ``/api/printer`` response.
TEMPERATURES = {
    "hotend.temperature": ("tool0", "actual"),
    "hotend.target": ("tool0", "target"),
    "bed.temperature": ("bed", "actual"),
    "bed.target": ("bed", "target"),
}


class ConfigError(Exception):
    pass


def is_loopback(host: str) -> bool:
    if host.lower() == "localhost":
        return True
    try:
        return ipaddress.ip_address(host.strip("[]")).is_loopback
    except ValueError:
        return False


def rfc3339(epoch: float) -> str:
    return datetime.fromtimestamp(epoch, timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


# --- HTTP -----------------------------------------------------------------------------


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    """A redirect would carry the API key to wherever it points."""

    def redirect_request(self, *args, **kwargs):
        return None


class OctoPrintClient:
    """Blocking GET-only client; the adapter calls it from a worker thread."""

    def __init__(self, base_url: str, api_key: str, timeout: float):
        self.base = base_url.rstrip("/")
        self._key = api_key
        self.timeout = timeout
        # No environment proxies (they would see the key) and no redirects.
        self._opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), _NoRedirect())
        #: Requests sent, as (method, path); the tests read it.
        self.sent: list = []
        #: Conformance fixture: behave as though the network were down.
        self.link_down = False

    def get(self, path: str, query: dict | None = None):
        """GET ``path`` and return ``(status, parsed JSON)``. Statuses OctoPrint uses to
        say something about the printer (409) come back; transport failures and
        unexpected statuses are classified :class:`AdapterError`."""
        if path not in GET_PATHS:
            raise AdapterError("internal", f"refusing to request {path}: not a read-only path")
        if self.link_down:
            raise AdapterError("disconnected", f"cannot reach {self.base}: network link is down (fixture)")
        url = self.base + path + (("?" + urlencode(query)) if query else "")
        request = urllib.request.Request(url, method="GET", headers={"X-Api-Key": self._key, "Accept": "application/json"})
        self.sent.append(("GET", path))
        try:
            with self._opener.open(request, timeout=self.timeout) as response:
                status, body = response.status, response.read(MAX_BODY + 1)
        except urllib.error.HTTPError as e:
            status, body = e.code, e.read(MAX_BODY + 1)
        except (socket.timeout, TimeoutError) as e:
            raise AdapterError("timeout", f"OctoPrint at {self.base} did not answer within {self.timeout:g} s") from e
        except urllib.error.URLError as e:
            if isinstance(e.reason, (socket.timeout, TimeoutError)):
                raise AdapterError("timeout", f"OctoPrint at {self.base} did not answer within {self.timeout:g} s") from e
            raise AdapterError("disconnected", f"cannot reach OctoPrint at {self.base}: {e.reason}") from e
        except OSError as e:
            raise AdapterError("disconnected", f"connection to OctoPrint at {self.base} failed: {e}") from e
        if len(body) > MAX_BODY:
            raise AdapterError("malformed_response", f"{path}: response is larger than {MAX_BODY} bytes")
        if status in (401, 403):
            raise AdapterError("device_failure", f"OctoPrint rejected the API key ({status}) on {path}")
        if status == 409:
            return status, _json(path, body, allow_empty=True)
        if status != 200:
            raise AdapterError("device_failure", f"OctoPrint answered {status} on {path}")
        return status, _json(path, body)


def _json(path: str, body: bytes, allow_empty: bool = False):
    try:
        value = json.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as e:
        if allow_empty:
            return None
        raise AdapterError("malformed_response", f"{path}: the response is not JSON") from e
    if not isinstance(value, dict):
        if allow_empty:
            return None
        raise AdapterError("malformed_response", f"{path}: expected a JSON object")
    return value


# --- Mapping OctoPrint's answers onto capabilities -------------------------------------


def connection_state(connection: dict) -> tuple:
    """``(state, source)`` from a ``/api/connection`` body. ``state`` is one of :data:`STATES`.
    ``source`` is "simulator" when the printer port is OctoPrint's Virtual Printer (the
    configured one while the connection is closed), otherwise "device"."""
    current = connection.get("current") if isinstance(connection, dict) else None
    if not isinstance(current, dict) or not isinstance(current.get("state"), str):
        raise AdapterError("malformed_response", "/api/connection: no current.state")
    text = current["state"].lower()
    options = connection.get("options") if isinstance(connection.get("options"), dict) else {}
    port = current.get("port") or options.get("portPreference")
    source = "simulator" if port == "VIRTUAL" else "device"
    if text.startswith(("closed", "offline")):
        return ("error" if "error" in text else "offline"), source  # "Closed with error", "Offline (Error: ...)"
    if text.startswith("error"):
        return "error", source
    if text.startswith(("printing", "starting", "transferring", "cancelling", "finishing", "resuming")):
        return "printing", source
    if text.startswith(("paused", "pausing")):
        return "paused", source
    if text.startswith("operational"):
        return "operational", source
    if text.startswith(("opening", "detecting", "connecting")):
        return "connecting", source
    raise AdapterError("malformed_response", f"/api/connection: unrecognised state {current['state']!r}")


def temperature(printer: dict, capability: str) -> tuple:
    """``(degrees Celsius, measured_at epoch seconds | None)`` for a temperature capability."""
    heater, field = TEMPERATURES[capability]
    block = printer.get("temperature") if isinstance(printer.get("temperature"), dict) else None
    entry = block.get(heater) if block else None
    value = entry.get(field) if isinstance(entry, dict) else None
    if block is not None and (heater not in block or (isinstance(entry, dict) and field in entry and value is None)):
        # Right after the printer connects, OctoPrint omits the heaters (seen with 1.11.8) or
        # reports null until the printer answers its first temperature poll. A printer
        # without a heated bed never reports "bed" at all.
        raise AdapterError("device_failure", f"OctoPrint reports no {heater} {field} temperature (not yet, or this printer has none)")
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        raise AdapterError("malformed_response", f"/api/printer: temperature.{heater}.{field} is {value!r}, not a number")
    measured = None
    history = block.get("history")
    if isinstance(history, list) and history and isinstance(history[-1], dict):
        t = history[-1].get("time")
        if isinstance(t, (int, float)) and not isinstance(t, bool) and math.isfinite(t):
            measured = float(t)
    return float(value), measured


# --- Configuration --------------------------------------------------------------------


class Settings:
    def __init__(self, config: dict, permissions: Permissions):
        unknown = set(config) - {"url", "api_key_env", "timeout_ms", "allow_plaintext", "descriptor"}
        if unknown:
            raise ConfigError(f"unknown config keys {sorted(unknown)}")
        self.url = config.get("url")
        parts = urlsplit(self.url) if isinstance(self.url, str) else None
        if parts is None or parts.scheme not in ("http", "https") or not parts.hostname:
            raise ConfigError("config.url must be an http:// or https:// URL")
        if parts.scheme == "http" and not is_loopback(parts.hostname) and not config.get("allow_plaintext", False):
            raise ConfigError(
                f"refusing to send an API key over plain HTTP to {parts.hostname}: use https://, "
                "or set allow_plaintext to accept it (docs/security/transport-security.md)"
            )
        permissions.check_network(self.url)
        name = config.get("api_key_env", "OCTOPRINT_API_KEY")
        if name not in os.environ or not os.environ[name]:
            raise ConfigError(f"the API key credential {name!r} is not set: list it under the manifest's credentials")
        self.api_key = os.environ[name]
        self.timeout = float(config.get("timeout_ms", 2000)) / 1000
        self.descriptor = config.get("descriptor")
        if not isinstance(self.descriptor, dict):
            raise ConfigError("config.descriptor must be the device descriptor")
        known = set(TEMPERATURES) | {"printer.state"}
        for cap in self.descriptor.get("capabilities", []):
            extra = set(cap.get("operations", [])) - READ_ONLY
            if extra:
                raise ConfigError(f"{cap.get('name')}: operations {sorted(extra)} are not supported; this adapter is read-only")
            if cap.get("name") not in known:
                raise ConfigError(f"{cap.get('name')}: this adapter cannot read that capability")


# --- The adapter ----------------------------------------------------------------------


class OctoPrintAdapter(Adapter):
    adapter_id = "openmhs.py.octoprint"
    version = "0.1.0"
    fixture_faults = ("link_down",)

    def __init__(self):
        self.settings: Settings | None = None
        self.client: OctoPrintClient | None = None

    async def initialize(self, config, permissions):
        await super().initialize(config, permissions)
        try:
            self.settings = Settings(config, permissions)
        except ConfigError as e:
            raise AdapterError("internal", f"config: {e}") from e
        s = self.settings
        self.client = OctoPrintClient(s.url, s.api_key, s.timeout)

    async def describe(self):
        return self.settings.descriptor

    async def _get(self, path: str, query: dict | None = None):
        return await asyncio.to_thread(self.client.get, path, query)

    async def connect(self):
        await self._get("/api/connection")

    async def health(self):
        try:
            _, body = await self._get("/api/connection")
            state, _source = connection_state(body)
        except AdapterError as e:
            if e.kind in ("timeout", "disconnected"):
                return ("disconnected", e.message)
            return ("degraded", e.message)
        if state in ("operational", "printing", "paused"):
            return "healthy"
        if state == "offline":
            return ("disconnected", "OctoPrint is reachable but its printer connection is closed")
        return ("degraded", f"OctoPrint reports the printer as {state}")

    async def read(self, capability):
        cap = next((c for c in self.settings.descriptor.get("capabilities", []) if c.get("name") == capability), None)
        if cap is None or "read" not in cap.get("operations", []):
            raise AdapterError("unsupported", f"no readable capability {capability!r}")
        _, connection = await self._get("/api/connection")
        state, source = connection_state(connection)
        if capability == "printer.state":
            return reading(state, source=source)
        if state == "offline":
            raise AdapterError("disconnected", "OctoPrint is reachable, but its printer connection is closed")
        status, printer = await self._get("/api/printer", {"history": "true", "limit": 1, "exclude": "sd,state"})
        if status == 409:
            raise AdapterError("disconnected", "OctoPrint says the printer is not operational")
        value, measured = temperature(printer, capability)
        return reading(
            {"value": value, "unit": "Cel"},
            timestamp=rfc3339(measured) if measured is not None else None,
            source=source,
        )

    async def execute(self, command, progress):
        raise AdapterError("unsupported", "the OctoPrint adapter is read-only: it sends nothing to the printer")

    async def fixture_inject(self, fault):
        if fault != "link_down":
            raise AdapterError("unsupported", f"cannot inject {fault}")
        self.client.link_down = True

    async def fixture_clear(self):
        self.client.link_down = False


if __name__ == "__main__":
    run(OctoPrintAdapter())
