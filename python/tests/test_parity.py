"""Invariant 10: every client path is the same gateway.

One scenario runs three times, each on a fresh gateway: through the `openmhs` CLI,
through `openmhs mcp` over stdio (a real MCP session with the real binary), and through
the Python SDK. Afterwards the operator collects every receipt and the whole audit log
from each gateway. Once ids, timestamps and hashes are normalized, all three must be
identical, record for record.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import unittest
from typing import Any, Callable

from support import AXIS, PSU, REPO, SETPOINT, Gateway, binary

sys.path.insert(0, str(REPO / "examples" / "agent-loop"))
from mcp_stdio import McpStdioClient  # noqa: E402

from openmhs import Client, Quantity  # noqa: E402

PRINCIPAL = "agent:parity"
WRITE = {"device": PSU, "capability": SETPOINT}

#: (step, kind, volts or None, idempotency key or None)
SCENARIO: list[tuple[str, str, float | None, str | None]] = [
    ("read", "read", None, None),
    ("check 2 V", "check", 2.0, None),
    ("48 V: out of range", "write", 48.0, "p1"),
    ("9 V: slew", "write", 9.0, "p2"),
    ("2 V: verified", "write", 2.0, "p3"),
    ("2 V again: idempotent replay", "write", 2.0, "p3"),
    ("home: no grant", "home", None, "p4"),
]


def via_sdk(gw: Gateway, token: str) -> list[str]:
    c = Client(gw.url, token)
    seen = []
    for _, kind, volts, key in SCENARIO:
        if kind == "read":
            seen.append(json.dumps(c.read(PSU, SETPOINT).value.to_dict(), sort_keys=True))
        elif kind == "check":
            req = {"target": {"device": PSU, "capability": SETPOINT}, "operation": "write",
                   "value": {"value": volts, "unit": "V"}}
            seen.append(c.preflight(req).decision.value)
        elif kind == "write":
            seen.append(c.write(PSU, SETPOINT, Quantity(volts, "V"), idempotency_key=key).state.value)
        else:
            seen.append(c.invoke(AXIS, "axis.home", idempotency_key=key).state.value)
    return seen


def via_cli(gw: Gateway, token: str) -> list[str]:
    env = {**os.environ, "OPENMHS_TOKEN": token}
    base = [str(binary()), "--home", str(gw.home), "--json"]

    def run(*args: str) -> Any:
        p = subprocess.run([*base, *args], env=env, capture_output=True, timeout=120)
        return json.loads(p.stdout.decode("utf-8"))

    seen = []
    for _, kind, volts, key in SCENARIO:
        if kind == "read":
            seen.append(json.dumps(run("read", PSU, SETPOINT)["state"]["value"], sort_keys=True))
        elif kind == "check":
            seen.append(run("exec", PSU, SETPOINT, "--value", f"{volts}V", "--check")["decision"])
        elif kind == "write":
            seen.append(run("exec", PSU, SETPOINT, "--value", f"{volts}V",
                            "--idempotency-key", key)["state"])
        else:
            seen.append(run("exec", AXIS, "axis.home", "--idempotency-key", key)["state"])
    return seen


def via_mcp(gw: Gateway, token: str) -> list[str]:
    env = {"OPENMHS_HOME": str(gw.home), "OPENMHS_TOKEN": token}
    seen = []
    with McpStdioClient([str(binary()), "mcp"], env) as mcp:
        for _, kind, volts, key in SCENARIO:
            if kind == "read":
                r = mcp.call("read_state", WRITE)
                seen.append(json.dumps(r.structured["value"], sort_keys=True))
            elif kind == "check":
                r = mcp.call("check_command", {**WRITE, "value": {"value": volts, "unit": "V"}})
                seen.append(r.structured["decision"])
            elif kind == "write":
                r = mcp.call("submit_command", {**WRITE, "value": {"value": volts, "unit": "V"},
                                                "idempotency_key": key})
                seen.append(r.structured["state"])
            else:
                r = mcp.call("submit_command", {"device": AXIS, "capability": "axis.home",
                                                "idempotency_key": key})
                seen.append(r.structured["state"])
    return seen


# ---------------------------------------------------------------------- normalization

TIMESTAMP = re.compile(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)")
HASH = re.compile(r"\b[0-9a-f]{64}\b")
#: Credential ids: 16 random hex digits since M8 (12 before, derived from the hash).
TOKEN_ID = re.compile(r"\btok_[0-9a-f]{12,16}\b")
#: Measured durations: real, and never equal across runs.
VOLATILE_KEYS = {"age_ms", "elapsed_ms", "duration_ms"}


def drop_volatile(v: Any) -> Any:
    if isinstance(v, dict):
        return {k: drop_volatile(x) for k, x in v.items() if k not in VOLATILE_KEYS}
    if isinstance(v, list):
        return [drop_volatile(x) for x in v]
    return v


def normalize(gw: Gateway) -> tuple[list[Any], list[Any]]:
    op = gw.operator()
    receipts = [r.to_dict() for r in reversed(op.commands(limit=500))]  # oldest first
    audit = [{"seq": e.seq, "kind": e.kind, "subject": e.subject, "data": e.data}
             for e in op.audit_tail(limit=5000)]
    text = json.dumps({"receipts": receipts, "audit": audit}, sort_keys=True)
    for i, r in enumerate(receipts):
        text = text.replace(r["command_id"], f"<cmd{i}>")
    text = TIMESTAMP.sub("<ts>", text)
    text = HASH.sub("<hash>", text)
    text = TOKEN_ID.sub("<token>", text)
    doc = drop_volatile(json.loads(text))
    return doc["receipts"], doc["audit"]


def scenario(path: Callable[[Gateway, str], list[str]]) -> tuple[list[str], list[Any], list[Any]]:
    with Gateway() as gw:
        op = gw.operator()
        op.add_grant({"id": "read-all", "principal": "agent:*", "device": "*",
                      "capability": "*", "operations": ["read"], "effect": "allow"})
        op.add_grant({"id": "psu-write", "principal": "agent:*", "device": PSU,
                      "capability": "*", "operations": ["write"], "effect": "allow"})
        token = op.create_token(PRINCIPAL, any_scope=True).token
        seen = path(gw, token)
        receipts, audit = normalize(gw)
        # The tokens themselves never appear in anything the gateway recorded.
        raw = json.dumps([r.to_dict() for r in op.commands(limit=500)]) + json.dumps(
            [e.to_dict() for e in op.audit_tail(limit=5000)])
        assert token not in raw and gw.operator_token not in raw
        return seen, receipts, audit


class Parity(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.runs = {name: scenario(fn) for name, fn in
                    [("sdk", via_sdk), ("cli", via_cli), ("mcp", via_mcp)]}

    def test_the_scenario_did_what_it_says(self) -> None:
        seen, receipts, _ = self.runs["sdk"]
        self.assertEqual(seen[1:], ["allow", "DENIED", "DENIED", "VERIFIED", "VERIFIED", "DENIED"])
        self.assertEqual(len(receipts), 4, "the replay must not create a command")
        codes = [r["policy"].get("errors", [{}])[0].get("code") for r in receipts]
        self.assertEqual(codes, ["command.value.out_of_range", "command.slew_exceeded",
                                 None, "policy.denied.no_grant"])
        self.assertTrue(all(r["principal"] == PRINCIPAL for r in receipts))

    def test_every_path_saw_the_same_results(self) -> None:
        sdk = self.runs["sdk"][0]
        for name in ("cli", "mcp"):
            with self.subTest(path=name):
                self.assertEqual(self.runs[name][0], sdk)

    def test_receipts_are_identical(self) -> None:
        sdk = self.runs["sdk"][1]
        for name in ("cli", "mcp"):
            with self.subTest(path=name):
                self.assertEqual(self.runs[name][1], sdk)

    def test_audit_records_are_identical(self) -> None:
        sdk = self.runs["sdk"][2]
        self.assertGreater(len(sdk), len(SCENARIO))
        for name in ("cli", "mcp"):
            with self.subTest(path=name):
                self.assertEqual(self.runs[name][2], sdk)


if __name__ == "__main__":
    unittest.main()
