"""The client against a real gateway process running the virtual lab."""

from __future__ import annotations

import socket
import threading
import time
import unittest
from unittest.mock import patch

from support import AXIS, PSU, SETPOINT, Gateway, default_grants, grant
from test_models import jsonschema, receipt_validator

from openmhs import (
    ApprovalStatus,
    Availability,
    Client,
    CommandState,
    Decision,
    GatewayError,
    GatewayUnreachableError,
    NotRunningError,
    Outcome,
    PlanDecision,
    PlanState,
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
        created = self.op.create_token("agent:repr", any_scope=True)
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

    def test_ask_first_then_submit_with_the_approval(self) -> None:
        grants = [
            grant("read-all", "*", "*", ["read"]),
            grant("psu-approve", PSU, SETPOINT, ["write"], effect="require_approval"),
        ]
        req = {
            "target": {"device": PSU, "capability": SETPOINT},
            "operation": "write",
            "value": {"value": 1, "unit": "V"},
        }
        with Gateway() as gw:
            agent, _ = gw.agent("agent:asks", grants)
            op = gw.operator()
            a = agent.request_approval(req)
            self.assertIs(a.status, ApprovalStatus.PENDING)
            self.assertEqual(a.extra, {})
            self.assertEqual([x.id for x in agent.approvals()], [a.id])
            with self.assertRaises(GatewayError):
                agent.approve_approval(a.id)
            self.assertTrue(op.approve_approval(a.id).usable)
            # Not the command that was approved: refused, and the approval survives.
            other = dict(req, value={"value": 2, "unit": "V"})
            self.assertTrue(agent.submit(other, approval_id=a.id).denied)
            self.assertTrue(agent.submit(req, approval_id=a.id).confirmed)
            self.assertIs(op.approval(a.id).status, ApprovalStatus.CONSUMED)
            again = agent.submit(req, approval_id=a.id)
            self.assertEqual(again.reasons[0].code, "approval.consumed")

    def test_deny_and_withdraw(self) -> None:
        grants = [grant("psu-approve", PSU, SETPOINT, ["write"], effect="require_approval")]
        req = {
            "target": {"device": PSU, "capability": SETPOINT},
            "operation": "write",
            "value": {"value": 1, "unit": "V"},
        }
        with Gateway() as gw:
            agent, _ = gw.agent("agent:denied", grants)
            op = gw.operator()
            denied = op.deny_approval(agent.request_approval(req).id, reason="not now")
            self.assertIs(denied.status, ApprovalStatus.DENIED)
            self.assertEqual(denied.reason, "not now")
            withdrawn = agent.revoke_approval(agent.request_approval(req).id)
            self.assertIs(withdrawn.status, ApprovalStatus.REVOKED)
            statuses = {a.status for a in op.approvals([ApprovalStatus.DENIED, "REVOKED"])}
            self.assertEqual(statuses, {ApprovalStatus.DENIED, ApprovalStatus.REVOKED})


class UnknownOutcome(unittest.TestCase):
    def test_an_operator_resolves_what_an_agent_cannot(self) -> None:
        with Gateway() as gw:
            agent, _ = gw.agent("agent:py")
            op = gw.operator()
            op.sim_fault(PSU, "hang")
            r = agent.write(PSU, SETPOINT, Quantity(1, "V"), deadline_ms=800)
            self.assertIs(r.state, CommandState.UNKNOWN_OUTCOME)
            with self.assertRaises(GatewayError) as e:
                agent.resolve(r.command_id, applied=False, reason="looks fine")
            self.assertEqual(e.exception.code, "gateway.forbidden")
            settled = op.resolve(r.command_id, applied=False, reason="panel shows 0 V")
            self.assertIs(settled.state, CommandState.RECONCILED_NOT_APPLIED)
            self.assertTrue(settled.transitions[-1].detail.endswith(": panel shows 0 V"))


class RecoverableExecution(unittest.TestCase):
    def test_lost_submit_response_recovers_receipt_without_resubmitting(self) -> None:
        with Gateway() as gw:
            agent, _ = gw.agent("agent:recoverable")
            original_submit = agent.submit
            calls = 0

            def lose_response(request, approval_id=None, *, timeout=None):
                nonlocal calls
                calls += 1
                original_submit(request, approval_id=approval_id, timeout=timeout)
                raise GatewayUnreachableError("simulated lost response")

            with patch.object(agent, "submit", side_effect=lose_response):
                run = agent.execute(_set(1.25))

            self.assertEqual(calls, 1)
            self.assertTrue(run.receipt.confirmed)
            self.assertEqual(run.receipt.idempotency_key, run.idempotency_key)
            self.assertIsNone(run.action_required)
            self.assertEqual(
                agent.command_by_key(PSU, run.idempotency_key).command_id,
                run.receipt.command_id,
            )
            self.assertIsNone(gw.operator().command_by_key(PSU, run.idempotency_key))

    def test_missing_status_is_retained_as_a_recoverable_handle(self) -> None:
        with Gateway() as gw:
            agent, _ = gw.agent("agent:not-admitted")
            with patch.object(agent, "submit", side_effect=GatewayUnreachableError("offline")):
                run = agent.execute(_set(1.5), idempotency_key="persist-me")

            self.assertIsNone(run.receipt)
            self.assertEqual(run.action_required, "check_status")
            self.assertIsNone(run.refresh())
            self.assertEqual(run.idempotency_key, "persist-me")

    def test_wait_is_bounded_and_reconcile_requires_uncertainty(self) -> None:
        with Gateway() as gw:
            agent, _ = gw.agent("agent:bounded")
            run = agent.execute(_set(1.75))
            self.assertTrue(run.receipt.confirmed)
            self.assertEqual(run.wait(timeout=0).command_id, run.receipt.command_id)
            with self.assertRaises(ValueError):
                run.reconcile()


class Credentials(unittest.TestCase):
    def test_a_credential_needs_scopes(self) -> None:
        with Gateway() as gw:
            op = gw.operator()
            for kwargs in ({}, {"scopes": []}, {"scopes": [], "any_scope": True}):
                with self.assertRaises(ValueError):
                    op.create_token("agent:none", **kwargs)

    def test_scopes_limit_and_rotation_keeps_them(self) -> None:
        with Gateway() as gw:
            op = gw.operator()
            for g in default_grants():
                op.add_grant(g)
            scope = {"device": PSU, "capability": "*", "operations": ["read"]}
            created = op.create_token("agent:narrow", scopes=[scope], ttl_ms=3_600_000)
            self.assertEqual(created.scopes, [scope])
            self.assertIsNotNone(created.expires_at)
            self.assertEqual(created.extra, {})
            narrow = Client(gw.url, created.token)
            info = narrow.info()
            self.assertEqual((info.credential_id, info.scopes), (created.id, [scope]))
            self.assertTrue(narrow.read(PSU, SETPOINT).usable)
            # Grants allow the write; the credential's scope does not.
            r = narrow.write(PSU, SETPOINT, Quantity(1, "V"), idempotency_key="narrow")
            self.assertEqual(r.reasons[0].code, "auth.scope.denied")
            with self.assertRaises(GatewayError):
                narrow.read(AXIS, "axis.position")

            rotated = op.rotate_token(created.id)
            self.assertEqual(rotated.scopes, [scope])
            self.assertNotEqual(rotated.token, created.token)
            with self.assertRaises(UnauthorizedError):
                narrow.info()
            self.assertEqual(Client(gw.url, rotated.token).info().principal, "agent:narrow")
            listed = {t.id: t for t in op.tokens()}
            self.assertIsNotNone(listed[created.id].revoked_at)
            self.assertEqual(listed[rotated.id].kind, "agent")


def _set(volts: float) -> dict:
    return {
        "target": {"device": PSU, "capability": SETPOINT},
        "operation": "write",
        "value": {"value": volts, "unit": "V"},
    }


class LeasesAndPlans(unittest.TestCase):
    def test_a_lease_keeps_others_off_until_released(self) -> None:
        with Gateway() as gw:
            a, _ = gw.agent("agent:holder")
            b, _ = gw.agent("agent:other", [])
            lease = a.acquire_lease(600_000, devices=[PSU], reason="calibration")
            self.assertEqual(lease.extra, {})
            self.assertIn(PSU, lease.claim.devices)
            refused = b.write(PSU, SETPOINT, Quantity(1, "V"))
            self.assertEqual(refused.reasons[0].code, "lease.held")
            self.assertEqual([x.id for x in b.leases(active=True)], [lease.id])
            released = a.release_lease(lease.id, reason="done")
            self.assertEqual(released.released_by, "agent:holder")
            self.assertTrue(b.write(PSU, SETPOINT, Quantity(1, "V")).confirmed)

    def test_an_offline_device_is_reported_and_refused(self) -> None:
        with Gateway() as gw:
            agent, _ = gw.agent("agent:bench")
            statuses = {s.device: s for s in agent.device_status()}
            self.assertEqual(statuses[PSU].availability, Availability.ONLINE)
            self.assertFalse(statuses[PSU].busy)
            self.assertEqual(statuses[PSU].extra, {})

            gw.operator().sim_fault(PSU, "link-down")
            time.sleep(1.1)  # a refresh probes a device at most once a second
            (psu,) = agent.device_status(PSU, refresh=True)
            self.assertTrue(psu.offline)
            refused = agent.write(PSU, SETPOINT, Quantity(1, "V"))
            self.assertEqual(refused.reasons[0].code, "device.unavailable")

            gw.operator().sim_fault(PSU, "clear")
            self.assertTrue(agent.write(PSU, SETPOINT, Quantity(1, "V")).confirmed)

    def test_a_plan_runs_and_its_preflight_changes_nothing(self) -> None:
        with Gateway() as gw:
            agent, _ = gw.agent("agent:planner")
            spec = {"steps": [
                {"id": "low", "request": _set(1.0)},
                {"id": "high", "after": ["low"], "request": _set(2.0)},
            ]}
            check = agent.preflight_plan(spec)
            self.assertTrue(check.ok)
            self.assertIs(check.outcome, PlanState.SUCCEEDED)
            self.assertEqual([s.verdict for s in check.steps], ["allowed", "allowed"])
            self.assertEqual(agent.plans(), [])
            plan = agent.submit_plan(spec)
            self.assertEqual(plan.extra, {})
            self.assertIs(plan.state, PlanState.SUCCEEDED)
            self.assertTrue(plan.state.terminal)
            receipt = agent.command(plan.step("high").commands[-1])
            self.assertTrue(receipt.confirmed)
            self.assertEqual([p.id for p in agent.plans()], [plan.id])

    def test_an_operator_decides_a_waiting_plan(self) -> None:
        grants = [grant("psu-approve", PSU, SETPOINT, ["write"], effect="require_approval")]
        with Gateway() as gw:
            agent, _ = gw.agent("agent:waits", grants)
            plan = agent.submit_plan({"steps": [{"id": "set", "request": _set(1.0)}]})
            self.assertIs(plan.state, PlanState.WAITING_APPROVAL)
            with self.assertRaises(GatewayError) as e:
                agent.decide_plan(plan.id, PlanDecision.ABORT)
            self.assertEqual(e.exception.code, "gateway.forbidden")
            ended = gw.operator().decide_plan(plan.id, "abort", reason="not now")
            self.assertIs(ended.state, PlanState.ABORTED)
            self.assertIs(ended.decisions[0].decision, PlanDecision.ABORT)
            held = agent.command(plan.step("set").commands[-1])
            self.assertIs(held.state, CommandState.CANCELLED)


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


class StateFabric(unittest.TestCase):
    """Watches and events through the Python client (ADR-0023)."""

    def test_a_watch_streams_samples_and_needs_a_subscribe_grant(self) -> None:
        measured = "power.output.voltage.measured"
        with Gateway() as gw:
            agent, _ = gw.agent("agent:watcher", [grant("read-all", "*", "*", ["read"])])
            with self.assertRaises(Exception):
                agent.create_watch(PSU, measured, 200, 2_000)
        with Gateway() as gw:
            agent, _ = gw.agent("agent:watcher", [
                grant("read-all", "*", "*", ["read"]),
                grant("psu-sub", PSU, "*", ["subscribe"]),
            ])
            start = agent.poll_events()
            w = agent.create_watch(PSU, measured, 200, 3_000)
            self.assertEqual([x["id"] for x in agent.watches()], [w["id"]])
            seen, cursor = 0, start["next"]
            for _ in range(10):
                b = agent.poll_events(cursor, stream=start["stream"], kinds=["observation"],
                                      wait_ms=1_000)
                self.assertEqual(b["missed"], 0)
                cursor = b["next"]
                seen += sum(1 for e in b["events"] if e["cause"] == "watch")
                if seen >= 3:
                    break
            self.assertGreaterEqual(seen, 3)
            agent.cancel_watch(w["id"])
            self.assertEqual(agent.watches(), [])


class Procedures(unittest.TestCase):
    """Procedures through the Python client (spec/procedures.md)."""

    def test_proposed_is_not_runnable_until_the_operator_approves(self) -> None:
        spec = {
            "name": "psu.set", "version": 1,
            "params": {"v": {"type": "quantity", "unit": "V", "min": 0, "max": 2}},
            "plan": {"steps": [{"id": "set", "request": {
                "target": {"device": PSU, "capability": SETPOINT},
                "operation": "write", "value": {"$param": "v"}}}]},
        }
        with Gateway() as gw:
            agent, _ = gw.agent("agent:proc", default_grants())
            self.assertEqual(agent.propose_procedure(spec)["status"], "proposed")
            with self.assertRaises(Exception):
                agent.run_procedure("psu.set", {"v": {"value": 1, "unit": "V"}})
            with self.assertRaises(Exception):
                agent.approve_procedure("psu.set", 1)
            gw.operator().approve_procedure("psu.set", 1)
            plan = agent.run_procedure("psu.set", {"v": {"value": 1, "unit": "V"}})
            self.assertEqual(plan["state"], "SUCCEEDED")
