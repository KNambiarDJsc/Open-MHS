"""The client against a real gateway process running the virtual lab."""

from __future__ import annotations

import socket
import threading
import time
import unittest

from support import AXIS, PSU, SETPOINT, Gateway, default_grants, grant
from test_models import jsonschema, receipt_validator

from openmhs import (
    Client,
    CommandState,
    Decision,
    GatewayError,
    GatewayUnreachableError,
    NotRunningError,
    Outcome,
    Quantity,
    Role,
    UnauthorizedError,
)


class LiveGateway(unittest.TestCase):
    gw: Gateway

    @classmethod
    def setUpClass(cls) -> None:
        cls.gw = Gateway().__enter__()
        cls.agent, cls.agent_token = cls.gw.agent("agent:py")
        cls.op = cls.gw.operator()

    @classmethod
    def tearDownClass(cls) -> None:
        cls.gw.__exit__(None, None, None)

    def test_identity_and_full_field_coverage(self) -> None:
        info = self.agent.info()
        self.assertEqual(info.principal, "agent:py")
        self.assertIs(info.role, Role.AGENT)
        self.assertTrue(info.simulation)
        # Nothing the gateway sends is unknown to the models.
        self.assertEqual(info.extra, {})
        self.assertIs(self.op.info().role, Role.OPERATOR)

    def test_discovery_and_reads(self) -> None:
        devices = self.agent.devices()
        self.assertEqual({d.id for d in devices}, {PSU, AXIS, "sim.thermo-01"})
        psu = self.agent.describe(PSU)
        self.assertEqual(psu.extra, {})
        self.assertEqual(psu.capability(SETPOINT)["value"]["unit"], "V")
        r = self.agent.read(PSU, SETPOINT)
        self.assertEqual(r.extra, {})
        self.assertTrue(r.usable)
        self.assertIsInstance(r.value, Quantity)
        self.assertEqual(r.value.unit, "V")

    def test_a_refusal_is_a_receipt_not_an_exception(self) -> None:
        r = self.agent.write(PSU, SETPOINT, Quantity(48, "V"), idempotency_key="py-range")
        self.assertTrue(r.denied)
        self.assertIs(r.state, CommandState.DENIED)
        self.assertFalse(r.confirmed)
        self.assertEqual(r.reasons[0].code, "command.value.out_of_range")
        self.assertEqual(r.reasons[0].details["max"], 12.0)
        self.assertEqual(r.extra, {})

    def test_preflight_runs_nothing(self) -> None:
        before = len(self.agent.commands())
        req = {
            "target": {"device": PSU, "capability": SETPOINT},
            "operation": "write",
            "value": {"value": 11, "unit": "V"},
        }
        p = self.agent.preflight(req)
        self.assertIs(p.decision, Decision.DENY)
        self.assertEqual(p.errors[0].code, "command.slew_exceeded")
        self.assertEqual(p.errors[0].details["unit"], "V")
        self.assertEqual(p.extra, {})
        self.assertEqual(len(self.agent.commands()), before)

    def test_verified_write_and_idempotency(self) -> None:
        r = self.agent.write(PSU, SETPOINT, Quantity(1, "V"), idempotency_key="py-ok")
        self.assertTrue(r.confirmed, r.to_dict())
        self.assertTrue(r.verification.matched)
        again = self.agent.write(PSU, SETPOINT, Quantity(1, "V"), idempotency_key="py-ok")
        self.assertEqual(again.command_id, r.command_id)
        with self.assertRaises(GatewayError) as cm:
            self.agent.write(PSU, SETPOINT, Quantity(1.5, "V"), idempotency_key="py-ok")
        self.assertEqual(cm.exception.code, "command.idempotency.conflict")
        self.assertEqual(cm.exception.command_id, r.command_id)
        self.assertEqual(self.agent.command(r.command_id).command_id, r.command_id)

    def test_no_grant_is_denied(self) -> None:
        r = self.agent.invoke(AXIS, "axis.home", idempotency_key="py-home")
        self.assertIs(r.outcome, Outcome.DENIED)
        self.assertEqual(r.reasons[0].code, "policy.denied.no_grant")

    def test_agents_cannot_use_operator_methods(self) -> None:
        for call in (self.agent.grants, self.agent.tokens, self.agent.audit_verify):
            with self.subTest(method=call.__name__):
                with self.assertRaises(GatewayError) as cm:
                    call()
                self.assertEqual(cm.exception.code, "gateway.forbidden")

    def test_bad_token(self) -> None:
        with self.assertRaises(UnauthorizedError):
            Client(self.gw.url, "omhs_wrong").info()

    def test_audit_chain_is_valid_and_records_preflight(self) -> None:
        self.agent.preflight(
            {"target": {"device": PSU, "capability": SETPOINT}, "operation": "write",
             "value": {"value": 0.5, "unit": "V"}}
        )
        v = self.op.audit_verify()
        self.assertTrue(v.valid)
        kinds = {e.kind for e in self.op.audit_tail(limit=500)}
        self.assertIn("command.preflight", kinds)

    def test_token_never_in_repr(self) -> None:
        self.assertNotIn(self.agent_token, repr(self.agent))
        created = self.op.create_token("agent:repr")
        self.assertNotIn(created.token, repr(created))

    @unittest.skipIf(jsonschema is None, "jsonschema not installed")
    def test_live_receipts_validate_against_the_schema(self) -> None:
        validator = receipt_validator()
        self.agent.write(PSU, SETPOINT, Quantity(0.5, "V"), idempotency_key="py-schema")
        receipts = self.agent.commands(limit=500)
        self.assertTrue(receipts)
        for r in receipts:
            validator.validate(r.to_dict())


class Approval(unittest.TestCase):
    def test_a_human_approves_while_the_agent_waits(self) -> None:
        grants = [
            grant("read-all", "*", "*", ["read"]),
            grant("psu-approve", PSU, SETPOINT, ["write"], effect="require_approval"),
        ]
        with Gateway() as gw:
            agent, _ = gw.agent("agent:py", grants)
            r = agent.write(PSU, SETPOINT, Quantity(1, "V"), idempotency_key="needs-ok")
            self.assertTrue(r.pending)
            self.assertIs(r.state, CommandState.AWAITING_APPROVAL)
            pending = agent.commands([CommandState.AWAITING_APPROVAL])
            self.assertEqual([p.command_id for p in pending], [r.command_id])
            approved = gw.operator().approve(r.command_id)
            self.assertTrue(approved.confirmed)
            self.assertEqual(approved.policy.approved_by, "operator:local")
            self.assertTrue(agent.command(r.command_id).confirmed)


class Discovery(unittest.TestCase):
    def test_from_home_finds_the_gateway(self) -> None:
        with Gateway() as gw:
            self.assertIs(Client.from_home(gw.home).info().role, Role.OPERATOR)
            gw.agent("agent:home", default_grants())

    def test_from_home_without_gateway(self) -> None:
        with self.assertRaises(NotRunningError):
            Client.from_home("/definitely/not/an/openmhs/home")

    def test_unreachable(self) -> None:
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            port = s.getsockname()[1]
        with self.assertRaises(GatewayUnreachableError):
            Client(f"http://127.0.0.1:{port}", "x", timeout=5).info()


class Concurrency(unittest.TestCase):
    def test_one_client_from_many_threads(self) -> None:
        with Gateway() as gw:
            agent, _ = gw.agent("agent:threads")
            errors: list[BaseException] = []

            def work() -> None:
                try:
                    for _ in range(5):
                        agent.read(PSU, SETPOINT)
                except BaseException as e:  # noqa: BLE001 - surfaced below
                    errors.append(e)

            threads = [threading.Thread(target=work) for _ in range(8)]
            start = time.monotonic()
            for t in threads:
                t.start()
            for t in threads:
                t.join(timeout=60)
            self.assertEqual(errors, [])
            self.assertLess(time.monotonic() - start, 60)


if __name__ == "__main__":
    unittest.main()
