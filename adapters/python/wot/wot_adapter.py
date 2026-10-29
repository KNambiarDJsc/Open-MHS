"""A generic Web of Things (HTTP) adapter for OpenMHS (experimental, simulator-tested).

It serves one Thing from a descriptor and *bindings* that ``td_import.py`` generated from
that Thing's Description and a human review. Nothing here is specific to a vendor or a
device: the same code drives OpenFlexure's microscope server (LabThings-FastAPI) and any
other HTTP Thing whose actions behave like WoT's (synchronous) or LabThings' (asynchronous
invocation resource).

What it does, and does not do:

* Reads are ``GET`` of a property's href. The value is bare JSON with no measurement time,
  so the reading is stamped with the time the adapter received it (``source`` says so in
  ``config.source``); OpenMHS's rule that a readback must follow the command (ADR-0020)
  then means "received after the command was sent".
* An action is started with ``POST``. A LabThings server answers ``201`` with an invocation
  resource (``status``, ``id``, ``href``): that is *acceptance*, not completion. The adapter
  tells OpenMHS the command was accepted, then polls the invocation until it is ``completed``,
  ``error`` or ``cancelled``, or the command's deadline passes.
* A deadline that passes with the invocation still ``pending`` or ``running`` ends as a
  ``timeout``, which OpenMHS records as ``UNKNOWN_OUTCOME`` and never retries. The error
  message names the invocation id and href so a person (or ``reconcile``) can find it. The
  adapter does **not** cancel it: a cancel request is not a stop, and the action may be
  mid-motion.
* A ``completed`` invocation is reported as applied. An ``error`` or ``cancelled`` one is
  reported as a failed command. Neither says how far the action got, so the descriptor's
  readback verification (when a reviewer declared one) is what establishes the state.
* Every request is checked against the manifest's ``endpoints.network``. There is no code
  path that sends ``PUT`` or ``DELETE``: writing a property or cancelling an invocation is
  not offered.
* An adapter-side ledger (``config.ledger``, JSON lines) records each action's command id and
  invocation id as they become known, so an invocation can be matched to its command even
  after a crash. It is the adapter's own file, not OpenMHS's journal.
"""

from __future__ import annotations

import asyncio
import json
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "python"))
from openmhs.adapter import (  # noqa: E402
    Adapter, Applied, AdapterError, Command, Permissions, Progress, Rejected, reading, run,
)

TERMINAL = {"completed", "error", "cancelled"}
MAX_BODY = 1 << 20


class WotAdapter(Adapter):
    adapter_id = "openmhs.py.wot"
    version = "0.1.0"
    fixture_faults = ()

    async def initialize(self, config: dict, permissions: Permissions) -> None:
        await super().initialize(config, permissions)
        self.descriptor: dict = config["descriptor"]
        self.bindings: dict = config["bindings"]
        self.timeout = float(config.get("timeout_ms", 2000)) / 1000
        self.poll = float(config.get("poll_ms", 100)) / 1000
        self.source = config.get("source", "device")
        self.ledger = config.get("ledger")
        for name, b in self.bindings.items():
            permissions.check_network(b["href"])
        if self.ledger:
            permissions.check_read(self.ledger)
        declared = {c["name"] for c in self.descriptor["capabilities"]}
        if set(self.bindings) != declared:
            raise AdapterError("internal", "bindings and descriptor name different capabilities")

    async def describe(self) -> dict:
        return self.descriptor

    # ------------------------------------------------------------------ HTTP

    def _request(self, method: str, url: str, body: Any = None) -> tuple[int, Any]:
        """One request. ``method`` is only ever GET or POST."""
        assert method in ("GET", "POST")
        self.permissions.check_network(url)
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(url, data=data, method=method,
                                     headers={"Content-Type": "application/json", "Accept": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:  # noqa: S310
                raw = resp.read(MAX_BODY + 1)
                status = resp.status
        except urllib.error.HTTPError as e:
            raw, status = e.read(MAX_BODY + 1), e.code
        if len(raw) > MAX_BODY:
            raise AdapterError("malformed_response", "response larger than 1 MiB")
        try:
            return status, (json.loads(raw) if raw else None)
        except ValueError:
            raise AdapterError("malformed_response", f"HTTP {status} body is not JSON") from None

    async def _call(self, method: str, url: str, body: Any = None) -> tuple[int, Any]:
        return await asyncio.to_thread(self._request, method, url, body)

    def _note(self, **fields: Any) -> None:
        if not self.ledger:
            return
        fields["at"] = datetime.now(timezone.utc).isoformat(timespec="milliseconds")
        with open(self.ledger, "a", encoding="utf-8") as f:
            f.write(json.dumps(fields) + "\n")

    # ------------------------------------------------------------------ lifecycle

    async def health(self):
        probe = next((b for b in self.bindings.values() if b["kind"] == "property"), None)
        if probe is None:
            return "healthy"
        try:
            status, _ = await self._call("GET", probe["href"])
        except AdapterError as e:
            return ("disconnected", e.message)
        except (urllib.error.URLError, OSError, TimeoutError) as e:
            return ("disconnected", str(getattr(e, "reason", e)))
        return "healthy" if status < 500 else ("degraded", f"HTTP {status}")

    async def connect(self) -> None:
        state = await self.health()
        if state != "healthy":
            raise AdapterError("disconnected", state[1] if isinstance(state, tuple) else "unreachable")

    async def read(self, capability: str) -> dict:
        b = self.bindings.get(capability)
        if b is None or b["kind"] != "property":
            raise AdapterError("unsupported", f"cannot read {capability!r}")
        try:
            status, body = await self._call("GET", b["href"])
        except (urllib.error.URLError, OSError, TimeoutError) as e:
            raise AdapterError("disconnected", str(getattr(e, "reason", e))) from None
        if status != 200:
            raise AdapterError("device_failure", f"HTTP {status} reading {capability}")
        if b.get("field") is not None:
            if not isinstance(body, dict) or b["field"] not in body:
                raise AdapterError("malformed_response", f"no field {b['field']!r} in the value")
            body = body[b["field"]]
        return reading(body, source=self.source)

    # ------------------------------------------------------------------ actions

    async def execute(self, command: Command, progress: Progress):
        b = self.bindings.get(command.capability)
        if b is None or b["kind"] != "action":
            raise AdapterError("unsupported", f"cannot execute {command.capability!r}")
        body = {}
        for name, value in command.params.items():
            body[b["wire"].get(name, name)] = value["value"] if isinstance(value, dict) and "value" in value else value
        deadline = datetime.fromisoformat(command.deadline.replace("Z", "+00:00")).timestamp()

        self._note(event="post", command_id=command.command_id, capability=command.capability, href=b["href"])
        try:
            status, reply = await self._call("POST", b["href"], body)
        except (urllib.error.URLError, OSError) as e:
            reason = getattr(e, "reason", e)
            refused = isinstance(reason, ConnectionRefusedError)
            self._note(event="post_failed", command_id=command.command_id, error=str(reason))
            if refused:
                raise AdapterError("not_sent", f"connection refused: {reason}") from None
            raise AdapterError("timeout", f"no answer to the action request ({reason}); it may have started") from None
        except TimeoutError:
            self._note(event="post_timeout", command_id=command.command_id)
            raise AdapterError("timeout", "no answer to the action request; it may have started") from None

        if status in (400, 404, 405, 409, 422):
            # The server refused the request before creating anything.
            self._note(event="refused", command_id=command.command_id, status=status)
            return Rejected(f"HTTP {status}: {json.dumps(reply)[:300]}")
        if status >= 500 and not (isinstance(reply, dict) and "status" in reply):
            raise AdapterError("device_failure", f"HTTP {status} starting the action")

        progress.accepted()
        if isinstance(reply, dict) and "status" in reply and ("href" in reply or "id" in reply):
            return await self._follow(command, b, reply, deadline)
        # A synchronous Thing: the answer is the result.
        self._note(event="sync_result", command_id=command.command_id, status=status)
        if status >= 400:
            return Rejected(f"HTTP {status}: {json.dumps(reply)[:300]}")
        return Applied(self._result(b, reply))

    def _result(self, b: dict, output: Any) -> dict:
        if not b.get("result"):
            return {}
        key = b.get("result_key")
        return {key: output} if key else (output if isinstance(output, dict) else {})

    async def _follow(self, command: Command, b: dict, reply: dict, deadline: float):
        inv_id = reply.get("id")
        href = reply.get("href") or ""
        self._note(event="accepted", command_id=command.command_id, invocation_id=inv_id, href=href,
                   status=reply.get("status"))
        state = reply
        while state.get("status") not in TERMINAL:
            if time.time() >= deadline:
                self._note(event="deadline", command_id=command.command_id, invocation_id=inv_id,
                           last_status=state.get("status"))
                raise AdapterError(
                    "timeout",
                    f"LabThings invocation {inv_id} ({href}) was still {state.get('status')} at the "
                    "command's deadline; it was NOT cancelled and may still be running",
                )
            await asyncio.sleep(self.poll)
            try:
                status, polled = await self._call("GET", href)
            except (urllib.error.URLError, OSError, TimeoutError) as e:
                self._note(event="poll_lost", command_id=command.command_id, invocation_id=inv_id,
                           error=str(getattr(e, "reason", e)))
                raise AdapterError(
                    "disconnected",
                    f"lost contact with invocation {inv_id} ({href}) after it was accepted; its outcome is unknown",
                ) from None
            if status == 404:
                self._note(event="invocation_gone", command_id=command.command_id, invocation_id=inv_id)
                raise AdapterError(
                    "disconnected",
                    f"invocation {inv_id} is no longer known to the server (expired or restarted); its outcome is unknown",
                )
            if status != 200 or not isinstance(polled, dict):
                raise AdapterError("malformed_response", f"unexpected answer polling invocation {inv_id}: HTTP {status}")
            state = polled
        self._note(event="terminal", command_id=command.command_id, invocation_id=inv_id,
                   status=state.get("status"), time_completed=state.get("timeCompleted"))
        if state["status"] == "completed":
            return Applied(self._result(b, state.get("output")))
        err = state.get("error") or {}
        detail = err.get("detail") or err.get("title") or state.get("status")
        verb = "was cancelled" if state["status"] == "cancelled" else "raised an error"
        return Rejected(
            f"invocation {inv_id} {verb} ({str(detail)[:200]}); how far the action got is not stated, "
            "so check the device's state before relying on it"
        )


if __name__ == "__main__":
    run(WotAdapter())
