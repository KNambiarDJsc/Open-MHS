"""One workflow step as one governed command: what the step may truthfully report.

This is the part of the MADSci node that decides what a step's status is, kept free of MADSci
so it can be tested deterministically, without a workcell. A step ends in one of three ways,
and each one is a claim:

``succeeded``
    OpenMHS confirmed the effect: ``VERIFIED``, or ``RECONCILED_APPLIED``.

``failed``
    A definite verdict: OpenMHS or the device said no, the command was withdrawn before it was
    sent, or OpenMHS concluded the command did not do what was asked. Never used for "we
    stopped waiting".

``unknown``
    Whether it happened is not known, or it may still happen. The step carries the command id,
    and a retry with the same key attaches to that command instead of sending another.

**The approval wait.** A command held for approval has not been sent, but it can still run:
an approver may decide at any time before its deadline. So a step cannot just stop waiting and
report ``failed``, because the transfer could then happen after the workflow has moved on, with
nothing watching. When the wait runs out the step *withdraws* the command
(``commands.cancel``), and reports according to what happened:

- withdrawn: ``CANCELLED``, so it can never run; ``failed``, and true.
- the approver got there first: the withdrawal is refused because the command moved on. The
  step follows it, within a bound, to its real outcome.
- the withdrawal got no answer: the command may still be approved, until its deadline, which
  the step set when it submitted. ``unknown``, and a retry attaches to it.

**Retries.** A retry never sends a second command for an effect that may already exist. It looks
the key up first. If the earlier attempt was definitely never sent (withdrawn, expired, or
denied by a check that may have changed), the retry submits a new attempt under a derived key,
so the journal keeps both and the record links them. A person's denial stays: a step denied
by an approver needs a new key, which is a new decision by whoever owns the workflow.
"""

from __future__ import annotations

import time
from dataclasses import dataclass, field
from typing import Any, Callable

from openmhs.errors import GatewayError, OpenMHSError

SUCCEEDED, FAILED, UNKNOWN = "succeeded", "failed", "unknown"

#: The effect is confirmed.
CONFIRMED = {"VERIFIED", "RECONCILED_APPLIED"}
#: Moving, or waiting for someone: the step can keep following these, within its bound.
IN_PROGRESS = {"CREATED", "VALIDATING", "APPROVED", "DISPATCHED", "ACCEPTED", "APPLIED"}
#: Whether the effect exists is open. ``UNVERIFIABLE``: the device reported it and nothing
#: could confirm it, which is not a success and certainly not a failure.
OPEN_OUTCOME = {"UNKNOWN_OUTCOME", "TIMED_OUT", "UNVERIFIABLE"}
#: A new attempt may follow these: the gateway's record shows nothing was sent.
NEVER_SENT = {"CANCELLED", "EXPIRED", "DENIED"}

MAX_ATTEMPTS = 16


@dataclass
class StepResult:
    status: str
    record: dict[str, Any]
    errors: list[tuple[str, str]] = field(default_factory=list)  # (error type, message)


def attempt_key(key: str, attempt: int) -> str:
    return key if attempt == 1 else f"{key}~{attempt}"


class GovernedStep:
    """Runs one idempotent step against an OpenMHS gateway. Every wait in it is bounded."""

    def __init__(
        self,
        client: Any,
        device: str,
        *,
        approval_wait_s: float = 60.0,
        settle_wait_s: float = 30.0,
        approval_grace_s: float = 30.0,
        poll_s: float = 0.5,
        submit_timeout_s: float = 60.0,
        call_timeout_s: float = 10.0,
        clock: Callable[[], float] = time.monotonic,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        self.client = client
        self.device = device
        self.approval_wait_s = approval_wait_s
        self.settle_wait_s = settle_wait_s
        self.approval_grace_s = approval_grace_s
        self.poll_s = poll_s
        self.submit_timeout_s = submit_timeout_s
        # A gateway that accepts a connection and never answers must not outlast a bound.
        self.call_timeout_s = call_timeout_s
        self.clock = clock
        self.sleep = sleep

    @property
    def deadline_ms(self) -> int:
        """The gateway's own bound on the command: past it, an approval cannot start it, even
        if this node has gone away. Longer than the node's wait, so the node withdraws first
        when it can; the grace leaves room for a transfer approved at the last moment."""
        return int((self.approval_wait_s + self.approval_grace_s) * 1000)

    def run(self, key: str, base_record: dict[str, Any], build_request: Callable[[], dict[str, Any]]) -> StepResult:
        """``build_request`` is only called if a new command is submitted: a retry attaches to
        the existing command, whatever the device's state has become since."""
        record = dict(base_record)
        try:
            existing, attempt, earlier = self._find(key)
        except GatewayError as e:
            return self._refused(record, e)
        except OpenMHSError as e:
            return StepResult(FAILED, record, [("OpenMHSUnreachable",
                                                f"could not ask OpenMHS, so this attempt sent nothing: {e}. If an "
                                                f"earlier attempt of {key} went unanswered, its outcome is still open")])
        record["earlier_attempts"] = earlier
        if existing is not None:
            return self._follow(existing, record)
        submitted_key = attempt_key(key, attempt)
        try:
            request = build_request()
        except OpenMHSError as e:  # read before anything was sent
            return StepResult(FAILED, record, [("OpenMHSUnreachable",
                                                f"could not ask OpenMHS, so this attempt sent nothing: {e}")])
        request = {**request, "idempotency_key": submitted_key, "deadline_ms": self.deadline_ms}
        record["idempotency_key"] = submitted_key
        try:
            receipt = self.client.call("commands.submit", {"request": request}, timeout=self.submit_timeout_s)
        except GatewayError as e:
            # The gateway answered: this never became a command.
            return self._refused(record, e)
        except OpenMHSError as e:
            # Sent, and no answer: it may have happened.
            return StepResult(UNKNOWN, record, [("OpenMHSOutcomeUnknown",
                                                 f"no answer from OpenMHS after submitting ({type(e).__name__}): look "
                                                 f"up {submitted_key} and reconcile it before retrying")])
        return self._follow(receipt, record)

    # ------------------------------------------------------------------ the key's history

    def _find(self, key: str) -> tuple[dict[str, Any] | None, int, list[dict[str, str]]]:
        """The command a retry should attach to, or else the attempt number to submit under,
        with the attempts that were never sent."""
        earlier: list[dict[str, str]] = []
        for attempt in range(1, MAX_ATTEMPTS + 1):
            try:
                receipt = self.client.call("commands.get_by_key",
                                           {"device": self.device, "idempotency_key": attempt_key(key, attempt)},
                                           timeout=self.call_timeout_s)
            except GatewayError as e:
                if e.code == "command.unknown":
                    return None, attempt, earlier
                raise
            if not self._new_attempt_may_follow(receipt):
                return receipt, attempt, earlier
            earlier.append({"command_id": receipt["command_id"], "state": receipt["state"]})
        # Past the bound, attach to the last one rather than keep creating commands.
        return receipt, MAX_ATTEMPTS, earlier[:-1]

    @staticmethod
    def _new_attempt_may_follow(receipt: dict[str, Any]) -> bool:
        if receipt["state"] not in NEVER_SENT:
            return False
        if any(t["state"] == "DISPATCHED" for t in receipt["transitions"]):
            return False  # the record says it was sent: never treat this as nothing happened
        codes = {e.get("code") for e in receipt.get("policy", {}).get("errors", [])}
        return "approval.denied" not in codes  # a person said no: that stands

    # ------------------------------------------------------------------ one command

    def _follow(self, receipt: dict[str, Any], record: dict[str, Any]) -> StepResult:
        """Follow a command, within bounds, to something this step can truthfully report."""
        command_id = receipt["command_id"]
        started = self.clock()
        withdrawn = False
        was_waiting = receipt["state"] == "AWAITING_APPROVAL"
        while True:
            state = receipt["state"]
            waiting = state == "AWAITING_APPROVAL"
            if was_waiting and not waiting:
                started = self.clock()  # approved: the time spent waiting was not execution
            was_waiting = waiting
            bound = self.approval_wait_s if waiting else self.settle_wait_s
            if not waiting and state not in IN_PROGRESS:
                return self._conclude(receipt, record)
            if self.clock() - started >= bound:
                if waiting and not withdrawn:
                    withdrawn = True
                    started = self.clock()  # what follows gets its own bound
                    outcome = self._withdraw(command_id, record)
                    if outcome is not None:
                        return outcome
                    latest = self._get(command_id)
                    if latest is None:
                        return self._lost(command_id, record)
                    receipt = latest
                    continue
                return self._held(receipt, record)
            self.sleep(self.poll_s)
            latest = self._get(command_id)
            if latest is None:
                return self._lost(command_id, record)
            receipt = latest

    def _get(self, command_id: str) -> dict[str, Any] | None:
        try:
            return self.client.call("commands.get", {"command_id": command_id}, timeout=self.call_timeout_s)
        except OpenMHSError:
            return None

    def _withdraw(self, command_id: str, record: dict[str, Any]) -> StepResult | None:
        """``None`` when the withdrawal was refused because the command moved on."""
        try:
            receipt = self.client.call("commands.cancel", {"command_id": command_id}, timeout=self.call_timeout_s)
        except GatewayError as e:
            if e.code == "command.invalid_state":
                return None  # the approver got there first: follow it
            return StepResult(UNKNOWN, self._describe({"command_id": command_id, "state": "AWAITING_APPROVAL",
                                                       "transitions": []}, record),
                              [("OpenMHSApprovalPending",
                                f"approval wait ran out and OpenMHS refused to withdraw command {command_id} "
                                f"[{e.code}]: it may still be approved until its deadline")])
        except OpenMHSError as e:
            return StepResult(UNKNOWN, self._describe({"command_id": command_id, "state": "AWAITING_APPROVAL",
                                                       "transitions": []}, record),
                              [("OpenMHSApprovalPending",
                                f"approval wait ran out and withdrawing command {command_id} got no answer "
                                f"({type(e).__name__}): it may still be approved until its deadline")])
        return StepResult(FAILED, self._describe(receipt, record),
                          [("OpenMHSApprovalNotObtained",
                            f"no approval within {self.approval_wait_s:g} s: command {command_id} was withdrawn, "
                            "was never sent and cannot run later. A retry asks for approval again")])

    def _lost(self, command_id: str, record: dict[str, Any]) -> StepResult:
        return StepResult(UNKNOWN, {**record, "command_id": command_id},
                          [("OpenMHSOutcomeUnknown",
                            f"lost contact with OpenMHS while following command {command_id}: it may have run or "
                            "may still run. Retry this step to attach to it, or reconcile it in OpenMHS")])

    def _held(self, receipt: dict[str, Any], record: dict[str, Any]) -> StepResult:
        return StepResult(UNKNOWN, self._describe(receipt, record),
                          [("OpenMHSOutcomeUnknown",
                            f"command {receipt['command_id']} is still {receipt['state']}: retry this step to attach "
                            "to it, or reconcile it in OpenMHS")])

    def _conclude(self, receipt: dict[str, Any], record: dict[str, Any]) -> StepResult:
        state = receipt["state"]
        described = self._describe(receipt, record)
        if state in CONFIRMED:
            return StepResult(SUCCEEDED, described)
        if state in OPEN_OUTCOME:
            return StepResult(UNKNOWN, described,
                              [("OpenMHSOutcomeUnknown",
                                f"OpenMHS command {receipt['command_id']} is {state}: it may have taken effect. "
                                "Reconcile it in OpenMHS before retrying this step")])
        errors = receipt.get("policy", {}).get("errors") or []
        adapter_error = (receipt.get("adapter_result") or {}).get("error")
        if adapter_error:
            errors = [*errors, adapter_error]
        return StepResult(FAILED, described,
                          [("OpenMHS", f"{e.get('code')}: {e.get('message', '')}") for e in errors]
                          or [("OpenMHS", f"{state}: {_last_detail(receipt)}")])

    def _refused(self, record: dict[str, Any], e: GatewayError) -> StepResult:
        return StepResult(FAILED, record, [("OpenMHSRefused", f"{e.code}: {e.message}")])

    @staticmethod
    def _describe(receipt: dict[str, Any], record: dict[str, Any]) -> dict[str, Any]:
        """Everything that ties this step to OpenMHS's evidence for it."""
        return {**record,
                "command_id": receipt["command_id"],
                "openmhs_state": receipt["state"],
                "idempotency_key": receipt.get("request", {}).get("idempotency_key", record.get("idempotency_key")),
                "approved_by": receipt.get("policy", {}).get("approved_by"),
                "deadline_at": receipt.get("deadline_at"),
                "request_params": receipt.get("request", {}).get("params"),
                "transitions": [t["state"] for t in receipt.get("transitions", [])]}


def _last_detail(receipt: dict[str, Any]) -> str:
    for t in reversed(receipt.get("transitions", [])):
        if t.get("detail"):
            return t["detail"]
    return ""
