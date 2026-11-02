"""What a MADSci step may report, deterministically and without MADSci.

``Scripted`` tests drive the step against a fake gateway and a fake clock, so timing is exact.
``RealGateway`` tests run the same step against a real ``openmhs serve --sim``: the state
machine they rely on is the gateway's, not the fake's.

    OPENMHS_BIN=... python -m unittest discover -s research/lab-automation/05_madsci/tests -v
"""

from __future__ import annotations

import sys
import threading
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve()
REPO = HERE.parents[4]
sys.path[:0] = [str(REPO / "python"), str(REPO / "python" / "tests"), str(HERE.parents[1] / "stack")]

from governed import FAILED, SUCCEEDED, UNKNOWN, GovernedStep, attempt_key  # noqa: E402
from openmhs.errors import GatewayError, GatewayUnreachableError  # noqa: E402

DEVICE = "dev"


def gone() -> GatewayUnreachableError:
    return GatewayUnreachableError("connection refused")


def receipt(state: str, *, command_id: str = "c1", key: str = "k", path: tuple = (), errors: list | None = None,
            adapter_error: dict | None = None) -> dict:
    return {"command_id": command_id, "state": state,
            "request": {"idempotency_key": key, "params": {}},
            "transitions": [{"state": s} for s in (*path, state)],
            "policy": {"errors": errors or [], "approved_by": None},
            "adapter_result": {"error": adapter_error} if adapter_error else None,
            "deadline_at": "2026-11-02T00:00:00Z"}


class Clock:
    def __init__(self) -> None:
        self.now = 0.0
        self.on_sleep = None

    def __call__(self) -> float:
        return self.now

    def sleep(self, s: float) -> None:
        self.now += s
        if self.on_sleep:
            self.on_sleep(self.now)


class FakeGateway:
    """Answers from a dictionary of receipts by key; tests change the receipts as time passes."""

    def __init__(self) -> None:
        self.by_key: dict[str, dict] = {}
        self.by_id: dict[str, dict] = {}
        self.calls: list[str] = []
        self.timeouts: list[object] = []
        self.submitted: list[dict] = []
        self.submit_result = None
        self.fail: dict[str, Exception] = {}
        self.cancel_result = None

    def put(self, r: dict) -> dict:
        self.by_id[r["command_id"]] = r
        self.by_key[r["request"]["idempotency_key"]] = r
        return r

    def call(self, method: str, params: dict | None = None, *, timeout: object = None) -> dict:
        self.calls.append(method)
        self.timeouts.append(timeout)
        if method in self.fail:
            raise self.fail[method]
        if method == "commands.get_by_key":
            found = self.by_key.get(params["idempotency_key"])
            if found is None:
                raise GatewayError("command.unknown", "no such command", -32000)
            return found
        if method == "commands.get":
            return self.by_id[params["command_id"]]
        if method == "commands.submit":
            self.submitted.append(params["request"])
            if isinstance(self.submit_result, Exception):
                raise self.submit_result
            return self.put({**self.submit_result, "request": params["request"]})
        if method == "commands.cancel":
            if isinstance(self.cancel_result, Exception):
                raise self.cancel_result
            return self.put(receipt("CANCELLED", command_id=params["command_id"],
                                    key=self.by_id[params["command_id"]]["request"]["idempotency_key"],
                                    path=("CREATED", "VALIDATING", "AWAITING_APPROVAL")))
        raise AssertionError(method)


def request() -> dict:
    return {"target": {"device": DEVICE, "capability": "x"}, "operation": "invoke", "params": {}}


class Scripted(unittest.TestCase):
    def setUp(self) -> None:
        self.gw = FakeGateway()
        self.clock = Clock()
        self.step = GovernedStep(self.gw, DEVICE, approval_wait_s=60, settle_wait_s=30, approval_grace_s=30, poll_s=1,
                                 clock=self.clock, sleep=self.clock.sleep)

    def run_step(self, key: str = "k"):
        return self.step.run(key, {"transfer_id": key}, request)

    def held(self) -> None:
        self.gw.submit_result = receipt("AWAITING_APPROVAL", path=("CREATED", "VALIDATING"))

    # ---- the known defect: a wait that runs out

    def test_a_wait_that_runs_out_withdraws_then_reports_failed_because_it_is_true(self) -> None:
        self.held()
        r = self.run_step()
        self.assertEqual(r.status, FAILED)
        self.assertEqual(r.errors[0][0], "OpenMHSApprovalNotObtained")
        self.assertIn("cannot run later", r.errors[0][1])
        self.assertEqual(r.record["openmhs_state"], "CANCELLED")
        self.assertIn("commands.cancel", self.gw.calls)
        self.assertGreaterEqual(self.clock.now, 60)
        self.assertLess(self.clock.now, 62, "the wait is bounded by approval_wait_s")

    def test_if_the_withdrawal_gets_no_answer_the_step_is_unknown_not_failed(self) -> None:
        self.held()
        self.gw.cancel_result = gone()
        r = self.run_step()
        self.assertEqual(r.status, UNKNOWN)
        self.assertEqual(r.errors[0][0], "OpenMHSApprovalPending")
        self.assertIn("may still be approved", r.errors[0][1])
        self.assertEqual(r.record["command_id"], "c1", "the command stays identified")

    def test_if_the_gateway_refuses_the_withdrawal_the_step_is_unknown(self) -> None:
        self.held()
        self.gw.cancel_result = GatewayError("policy.cancel.not_allowed", "no", -32000)
        self.assertEqual(self.run_step().status, UNKNOWN)

    def test_an_approval_that_beats_the_withdrawal_is_followed_to_its_real_outcome(self) -> None:
        self.held()
        self.gw.cancel_result = GatewayError("command.invalid_state", "moved on", -32000)

        def approved_at_the_last_moment(now: float) -> None:
            if now >= 60:
                self.gw.put(receipt("DISPATCHED", path=("AWAITING_APPROVAL", "APPROVED")))
            if now >= 63:
                r = receipt("VERIFIED", path=("AWAITING_APPROVAL", "APPROVED", "DISPATCHED"))
                r["policy"]["approved_by"] = "operator"
                self.gw.put(r)

        self.clock.on_sleep = approved_at_the_last_moment
        r = self.run_step()
        self.assertEqual(r.status, SUCCEEDED)
        self.assertEqual(r.record["approved_by"], "operator")

    def test_time_spent_waiting_for_approval_does_not_count_against_execution(self) -> None:
        self.held()

        def later(now: float) -> None:
            if now >= 50:  # approved after longer than settle_wait_s
                self.gw.put(receipt("DISPATCHED", path=("AWAITING_APPROVAL", "APPROVED")))
            if now >= 53:
                self.gw.put(receipt("VERIFIED", path=("AWAITING_APPROVAL", "APPROVED", "DISPATCHED")))

        self.clock.on_sleep = later
        self.assertEqual(self.run_step().status, SUCCEEDED)

    def test_a_command_still_running_past_its_bound_is_unknown_and_the_wait_ends(self) -> None:
        self.gw.submit_result = receipt("DISPATCHED")
        r = self.run_step()
        self.assertEqual(r.status, UNKNOWN)
        self.assertLessEqual(self.clock.now, 31)

    def test_the_gateways_deadline_outlives_the_nodes_wait_and_is_sent(self) -> None:
        self.held()
        self.run_step()
        self.assertEqual(self.gw.submitted[0]["deadline_ms"], 90_000)

    def test_every_call_to_the_gateway_has_its_own_timeout(self) -> None:
        # A gateway that accepts a connection and never answers must not outlast the bounds above.
        self.held()
        self.run_step()
        self.assertTrue({"commands.get_by_key", "commands.submit", "commands.get", "commands.cancel"}
                        <= set(self.gw.calls))
        self.assertNotIn(None, self.gw.timeouts)

    # ---- losing the gateway

    def test_gateway_lost_during_the_wait_is_unknown_not_failed(self) -> None:
        self.held()

        def lost(now: float) -> None:
            if now >= 5:
                self.gw.fail["commands.get"] = gone()

        self.clock.on_sleep = lost
        r = self.run_step()
        self.assertEqual(r.status, UNKNOWN)
        self.assertEqual(r.record["command_id"], "c1")

    def test_submit_with_no_answer_is_unknown(self) -> None:
        self.gw.submit_result = gone()
        r = self.run_step()
        self.assertEqual(r.status, UNKNOWN)
        self.assertIn("look up k", r.errors[0][1])

    def test_gateway_down_before_anything_is_sent_is_failed_and_says_so(self) -> None:
        self.gw.fail["commands.get_by_key"] = gone()
        r = self.run_step()
        self.assertEqual(r.status, FAILED)
        self.assertIn("this attempt sent nothing", r.errors[0][1])
        self.assertEqual(self.gw.submitted, [])

    def test_a_gateway_answer_refusing_the_request_is_failed_not_unknown(self) -> None:
        self.gw.submit_result = GatewayError("command.schema", "bad", -32602)
        r = self.run_step()
        self.assertEqual(r.status, FAILED)
        self.assertIn("command.schema", r.errors[0][1])

    # ---- outcomes

    def test_rejection_denial_and_failure_are_failed_with_their_reasons(self) -> None:
        for state, kw in (("REJECTED", {"adapter_error": {"code": "adapter.rejected", "message": "no tips"}}),
                          ("DENIED", {"errors": [{"code": "policy.denied.no_grant", "message": "no"}]}),
                          ("FAILED", {"adapter_error": {"code": "adapter.device_failure", "message": "x"}}),
                          ("VERIFICATION_FAILED", {}), ("RECONCILED_NOT_APPLIED", {})):
            with self.subTest(state):
                gw = FakeGateway()
                gw.submit_result = receipt(state, **kw)
                r = GovernedStep(gw, DEVICE, clock=self.clock, sleep=self.clock.sleep).run("k", {}, request)
                self.assertEqual(r.status, FAILED)
                self.assertTrue(r.errors)

    def test_confirmed_and_open_states(self) -> None:
        for state, want in (("VERIFIED", SUCCEEDED), ("RECONCILED_APPLIED", SUCCEEDED), ("UNKNOWN_OUTCOME", UNKNOWN),
                            ("TIMED_OUT", UNKNOWN), ("UNVERIFIABLE", UNKNOWN)):
            with self.subTest(state):
                gw = FakeGateway()
                gw.submit_result = receipt(state)
                r = GovernedStep(gw, DEVICE, clock=self.clock, sleep=self.clock.sleep).run("k", {}, request)
                self.assertEqual(r.status, want)

    # ---- retries

    def test_a_retry_attaches_and_never_rebuilds_or_resends(self) -> None:
        for state, want in (("UNKNOWN_OUTCOME", UNKNOWN), ("VERIFIED", SUCCEEDED), ("RECONCILED_APPLIED", SUCCEEDED),
                            ("REJECTED", FAILED)):
            with self.subTest(state):
                gw = FakeGateway()
                gw.put(receipt(state))

                def must_not_build() -> dict:
                    raise AssertionError("a retry must not build a new request")

                r = GovernedStep(gw, DEVICE, clock=self.clock, sleep=self.clock.sleep).run("k", {}, must_not_build)
                self.assertEqual(r.status, want)
                self.assertEqual(gw.submitted, [])

    def test_a_retry_while_held_attaches_to_the_same_approval(self) -> None:
        self.gw.put(receipt("AWAITING_APPROVAL", path=("CREATED", "VALIDATING")))

        def approved(now: float) -> None:
            if now >= 3:
                self.gw.put(receipt("VERIFIED", path=("AWAITING_APPROVAL", "APPROVED", "DISPATCHED")))

        self.clock.on_sleep = approved
        self.assertEqual(self.run_step().status, SUCCEEDED)
        self.assertEqual(self.gw.submitted, [])

    def test_after_a_withdrawal_a_retry_is_a_new_linked_attempt(self) -> None:
        self.gw.put(receipt("CANCELLED", path=("AWAITING_APPROVAL",)))
        self.gw.submit_result = receipt("VERIFIED", command_id="c2", key="k~2")
        r = self.run_step()
        self.assertEqual(r.status, SUCCEEDED)
        self.assertEqual(self.gw.submitted[0]["idempotency_key"], "k~2")
        self.assertEqual(r.record["earlier_attempts"], [{"command_id": "c1", "state": "CANCELLED"}])
        self.assertEqual(r.record["command_id"], "c2")

    def test_an_expired_or_checked_out_attempt_may_be_retried(self) -> None:
        for state, errors in (("EXPIRED", []), ("DENIED", [{"code": "command.outcome_unresolved", "message": "m"}])):
            with self.subTest(state):
                gw = FakeGateway()
                gw.put(receipt(state, errors=errors))
                gw.submit_result = receipt("VERIFIED", command_id="c2", key="k~2")
                r = GovernedStep(gw, DEVICE, clock=self.clock, sleep=self.clock.sleep).run("k", {}, request)
                self.assertEqual(gw.submitted[0]["idempotency_key"], "k~2")
                self.assertEqual(r.status, SUCCEEDED)

    def test_a_persons_denial_stands_on_retry(self) -> None:
        self.gw.put(receipt("DENIED", path=("AWAITING_APPROVAL",),
                            errors=[{"code": "approval.denied", "message": "wrong plate"}]))
        r = self.run_step()
        self.assertEqual(r.status, FAILED)
        self.assertEqual(self.gw.submitted, [])

    def test_anything_recorded_as_sent_is_never_treated_as_never_sent(self) -> None:
        self.gw.put(receipt("EXPIRED", path=("APPROVED", "DISPATCHED")))
        self.run_step()
        self.assertEqual(self.gw.submitted, [], "a record that says DISPATCHED wins over the final state")

    def test_a_retry_after_reconciliation_succeeds_from_the_existing_command(self) -> None:
        self.gw.put(receipt("RECONCILED_APPLIED", path=("DISPATCHED", "TIMED_OUT", "UNKNOWN_OUTCOME")))
        r = self.run_step()
        self.assertEqual(r.status, SUCCEEDED)
        self.assertEqual(self.gw.submitted, [])

    def test_retries_stop_creating_commands_at_the_bound(self) -> None:
        for n in range(1, 40):
            self.gw.put(receipt("CANCELLED", command_id=f"c{n}", key=attempt_key("k", n), path=("AWAITING_APPROVAL",)))
        r = self.run_step()
        self.assertEqual(self.gw.submitted, [])
        self.assertEqual(r.status, FAILED)


class RealGateway(unittest.TestCase):
    """The same step on a real gateway: withdrawal, late approval, denial, restartless retry."""

    @classmethod
    def setUpClass(cls) -> None:
        from support import PSU, Gateway, grant

        cls.PSU = PSU
        cls.LIMIT = "power.output.current.limit"  # no slew limit: tests can pick any value
        cls.gateway = Gateway()
        cls.gateway.__enter__()
        cls.operator = cls.gateway.operator()
        cls.agent, _ = cls.gateway.agent("agent:madsci", [
            grant("read", PSU, "*", ["read"]),
            grant("held", PSU, cls.LIMIT, ["write"], effect="require_approval"),
        ])

    @classmethod
    def tearDownClass(cls) -> None:
        cls.gateway.__exit__(None, None, None)

    def request(self, amps: float) -> dict:
        return {"target": {"device": self.PSU, "capability": self.LIMIT}, "operation": "write",
                "value": {"value": amps, "unit": "A"}}

    def await_held(self, key: str):
        """Once it has settled into waiting: it passes through CREATED and VALIDATING first."""
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            found = self.agent.command_by_key(self.PSU, key)
            if found is not None and found.to_dict()["state"] == "AWAITING_APPROVAL":
                return found
            time.sleep(0.02)
        self.fail(f"{key} was not held for approval within 10 s")

    def step(self, wait: float) -> GovernedStep:
        return GovernedStep(self.agent, self.PSU, approval_wait_s=wait, settle_wait_s=10, approval_grace_s=30,
                            poll_s=0.05)

    def test_withdrawn_late_approval_refused_retry_runs_once(self) -> None:
        first = self.step(0.3).run("real-1", {}, lambda: self.request(0.5))
        self.assertEqual(first.status, FAILED, first.errors)
        withdrawn = first.record["command_id"]
        self.assertEqual(first.record["openmhs_state"], "CANCELLED")
        with self.assertRaises(GatewayError) as late:
            self.operator.call("commands.approve", {"command_id": withdrawn})
        self.assertEqual(late.exception.code, "command.invalid_state")
        state = self.operator.command(withdrawn).to_dict()
        self.assertNotIn("DISPATCHED", [t["state"] for t in state["transitions"]])

        out: dict = {}
        worker = threading.Thread(target=lambda: out.setdefault(
            "r", self.step(15).run("real-1", {}, lambda: self.request(0.5))))
        worker.start()
        second = self.await_held("real-1~2")
        self.operator.call("commands.approve", {"command_id": second.command_id})
        worker.join(20)
        self.assertEqual(out["r"].status, SUCCEEDED, out["r"].errors)
        self.assertEqual(out["r"].record["earlier_attempts"], [{"command_id": withdrawn, "state": "CANCELLED"}])
        self.assertTrue(out["r"].record["approved_by"].startswith("operator"))
        self.assertTrue(self.operator.audit_verify().to_dict()["valid"])

    def test_a_denial_is_failed_and_stands(self) -> None:
        out: dict = {}
        worker = threading.Thread(target=lambda: out.setdefault(
            "r", self.step(15).run("real-2", {}, lambda: self.request(0.6))))
        worker.start()
        held = self.await_held("real-2")
        approval = next(a for a in self.operator.call("approvals.list", {}) if a.get("command_id") == held.command_id)
        self.operator.call("approvals.deny", {"id": approval["id"], "reason": "no"})
        worker.join(20)
        self.assertEqual(out["r"].status, FAILED)
        self.assertIn("approval.denied", str(out["r"].errors))
        again = self.step(0.2).run("real-2", {}, lambda: self.request(0.6))
        self.assertEqual(again.status, FAILED)
        self.assertIsNone(self.agent.command_by_key(self.PSU, "real-2~2"))

    def test_a_deadline_binds_even_if_the_node_disappears(self) -> None:
        # The node is gone and never withdraws: the gateway's own deadline ends the command.
        step = GovernedStep(self.agent, self.PSU, approval_wait_s=0.0, approval_grace_s=0.2, poll_s=0.01)
        request = {**self.request(0.7), "idempotency_key": "real-3", "deadline_ms": step.deadline_ms}
        held = self.agent.call("commands.submit", {"request": request})
        self.assertEqual(held["state"], "AWAITING_APPROVAL")
        time.sleep(0.4)
        after = self.operator.call("commands.approve", {"command_id": held["command_id"]})
        self.assertEqual(after["state"], "EXPIRED")
        self.assertNotIn("DISPATCHED", [t["state"] for t in after["transitions"]])

    def test_a_receipt_past_its_deadline_does_not_still_say_awaiting_approval(self) -> None:
        request = {**self.request(0.8), "idempotency_key": "real-4", "deadline_ms": 150}
        held = self.agent.call("commands.submit", {"request": request})
        self.assertEqual(held["state"], "AWAITING_APPROVAL")
        time.sleep(0.3)
        self.assertEqual(self.agent.call("commands.get", {"command_id": held["command_id"]})["state"], "EXPIRED")
        self.assertEqual(self.agent.command_by_key(self.PSU, "real-4").to_dict()["state"], "EXPIRED")


if __name__ == "__main__":
    unittest.main()
