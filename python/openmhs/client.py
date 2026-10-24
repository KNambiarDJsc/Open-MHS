"""A synchronous client for the gateway's JSON-RPC API (spec/api.md).

Standard library only. Like the CLI and the MCP server, it is a client of the gateway:
it never talks to hardware, so everything it does is authorized, validated, journaled
and audited there.
"""

from __future__ import annotations

import itertools
import json
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Iterable, Mapping

from .errors import (
    GatewayError,
    GatewayUnreachableError,
    NotRunningError,
    ProtocolError,
    UnauthorizedError,
)
from .models import (
    Approval,
    ApprovalStatus,
    AuditEntry,
    AuditVerifyResult,
    CommandState,
    CreatedToken,
    Descriptor,
    ErrorObject,
    GatewayInfo,
    PreflightResult,
    Quantity,
    ReadResult,
    Receipt,
    Role,
    TokenInfo,
    dump_value,
)
from .runtime import RuntimeInfo, default_home

RPC_PATH = "/rpc"


#: A scope that reaches everything; grants still decide (``create_token(any_scope=True)``).
ANY_SCOPE: Mapping[str, Any] = {
    "device": "*",
    "capability": "*",
    "operations": ["read", "write", "subscribe", "invoke"],
}


def _with_reason(approval_id: str, reason: str | None) -> dict[str, Any]:
    params: dict[str, Any] = {"id": approval_id}
    if reason is not None:
        params["reason"] = reason
    return params


class Client:
    """One gateway, one token.

    >>> gw = Client.from_home(token=os.environ["OPENMHS_TOKEN"])   # doctest: +SKIP
    >>> r = gw.write("sim.psu-01", "power.output.voltage.setpoint", Quantity(5, "V"),
    ...              idempotency_key="step-1")                       # doctest: +SKIP
    >>> r.confirmed                                                   # doctest: +SKIP
    True
    """

    def __init__(self, url: str, token: str, *, timeout: float | None = None) -> None:
        """``timeout`` bounds each HTTP call. The default (none) suits ``submit``,
        which waits for the command's own deadline, enforced by the gateway."""
        self._url = url.rstrip("/") + RPC_PATH
        self._token = token
        self._timeout = timeout
        self._ids = itertools.count(1)
        # Loopback by design: ignore any HTTP(S)_PROXY in the environment.
        self._opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def __repr__(self) -> str:  # never print the token
        return f"Client(url={self._url!r})"

    @classmethod
    def from_home(
        cls, home: Path | str | None = None, token: str | None = None, **kwargs: Any
    ) -> Client:
        """Find the running gateway through its runtime file. Without ``token`` this
        uses the operator token in that file, which only the gateway's user can read."""
        home = Path(home) if home is not None else default_home()
        try:
            info = RuntimeInfo.load(home)
        except (OSError, ValueError, KeyError) as e:
            raise NotRunningError(
                f"no running gateway found ({home}: {e}); start one with `openmhs serve`"
            ) from e
        return cls(info.url, token if token is not None else info.token, **kwargs)

    # -------------------------------------------------------------- transport

    def call(self, method: str, params: Mapping[str, Any] | None = None) -> Any:
        """Call any method and return the raw JSON result."""
        body = json.dumps(
            {"jsonrpc": "2.0", "id": next(self._ids), "method": method, "params": params or {}}
        ).encode("utf-8")
        req = urllib.request.Request(
            self._url,
            data=body,
            method="POST",
            headers={
                "Content-Type": "application/json",
                "Authorization": f"Bearer {self._token}",
                "User-Agent": "openmhs-python",
            },
        )
        try:
            with self._opener.open(req, timeout=self._timeout) as resp:
                raw = resp.read()
        except urllib.error.HTTPError as e:
            if e.code == 401:
                raise UnauthorizedError("unauthorized: the token was rejected") from None
            raw = e.read()  # JSON-RPC errors may come with a non-2xx status
        except (urllib.error.URLError, OSError) as e:
            raise GatewayUnreachableError(f"cannot reach the gateway at {self._url}: {e}") from e
        try:
            doc = json.loads(raw)
        except ValueError as e:
            raise ProtocolError(f"unexpected response: {e}") from e
        if not isinstance(doc, dict):
            raise ProtocolError("unexpected response: not a JSON object")
        err = doc.get("error")
        if err is not None:
            data = err.get("data") or {}
            raise GatewayError(
                code=data.get("code", str(err.get("code"))),
                message=err.get("message", ""),
                rpc_code=err.get("code", 0),
                errors=[ErrorObject.from_dict(x) for x in data.get("errors", [])],
                command_id=data.get("command_id"),
            )
        if "result" not in doc:
            raise ProtocolError("response has neither result nor error")
        return doc["result"]

    # -------------------------------------------------------------- devices

    def info(self) -> GatewayInfo:
        return GatewayInfo.from_dict(self.call("gateway.info"))

    def devices(self) -> list[Descriptor]:
        return [Descriptor.from_dict(d) for d in self.call("devices.list")]

    def describe(self, device: str) -> Descriptor:
        return Descriptor.from_dict(self.call("devices.describe", {"device": device}))

    def read(self, device: str, capability: str) -> ReadResult:
        """Read now. Authorized and audited. Check ``usable`` before relying on it."""
        return ReadResult.from_dict(
            self.call("state.read", {"device": device, "capability": capability})
        )

    # -------------------------------------------------------------- commands

    def submit(self, request: Mapping[str, Any], approval_id: str | None = None) -> Receipt:
        """Submit a raw command request (command.schema.json) and wait for its receipt:
        terminal, ``UNKNOWN_OUTCOME`` or ``AWAITING_APPROVAL``. With ``approval_id``, spend
        an approved request for exactly this command (see ``request_approval``)."""
        params: dict[str, Any] = {"request": dict(request)}
        if approval_id is not None:
            params["approval_id"] = approval_id
        return Receipt.from_dict(self.call("commands.submit", params))

    def write(
        self,
        device: str,
        capability: str,
        value: Quantity | int | float | bool | str,
        **options: Any,
    ) -> Receipt:
        """Write a property. ``options``: ``idempotency_key``, ``correlation_id``,
        ``deadline_ms``."""
        return self.submit(_request(device, capability, "write", value=value, **options))

    def invoke(
        self,
        device: str,
        capability: str,
        params: Mapping[str, Any] | None = None,
        **options: Any,
    ) -> Receipt:
        """Invoke an action with named parameters."""
        return self.submit(_request(device, capability, "invoke", params=params, **options))

    def preflight(self, request: Mapping[str, Any]) -> PreflightResult:
        """Would this be allowed right now? Same checks as a submission; runs nothing."""
        return PreflightResult.from_dict(
            self.call("commands.preflight", {"request": dict(request)})
        )

    def command(self, command_id: str) -> Receipt:
        return Receipt.from_dict(self.call("commands.get", {"command_id": command_id}))

    def commands(
        self, states: Iterable[CommandState | str] = (), limit: int | None = None
    ) -> list[Receipt]:
        """Receipts, newest first. Agents see only their own."""
        params: dict[str, Any] = {}
        st = [s.value if isinstance(s, CommandState) else s for s in states]
        if st:
            params["states"] = st
        if limit is not None:
            params["limit"] = limit
        return [Receipt.from_dict(r) for r in self.call("commands.list", params)]

    def approve(self, command_id: str) -> Receipt:
        """Operator only. Runs the command and returns its receipt."""
        return Receipt.from_dict(self.call("commands.approve", {"command_id": command_id}))

    def cancel(self, command_id: str) -> Receipt:
        return Receipt.from_dict(self.call("commands.cancel", {"command_id": command_id}))

    def reconcile(self, command_id: str) -> Receipt:
        """Settle an ``UNKNOWN_OUTCOME`` command by reading the device. Never re-sends."""
        return Receipt.from_dict(self.call("commands.reconcile", {"command_id": command_id}))

    # -------------------------------------------------------------- operator

    def grants(self) -> list[dict[str, Any]]:
        return list(self.call("grants.list"))

    def add_grant(self, grant: Mapping[str, Any]) -> dict[str, Any]:
        return dict(self.call("grants.add", {"grant": dict(grant)}))

    def revoke_grant(self, grant_id: str) -> dict[str, Any]:
        return dict(self.call("grants.revoke", {"id": grant_id}))

    def create_token(
        self,
        principal: str,
        role: Role | str = Role.AGENT,
        *,
        scopes: Iterable[Mapping[str, Any]] | None = None,
        any_scope: bool = False,
        ttl_ms: int | None = None,
    ) -> CreatedToken:
        """Issue a credential (operator). It reaches only its ``scopes`` (each
        ``{"device", "capability", "operations"}``, patterns allowed), and only what grants
        also allow. ``any_scope=True`` leaves the limit to the grants. One of the two is
        required: a credential without scopes could do nothing."""
        if any_scope == (scopes is not None):
            raise ValueError("pass scopes=[...] or any_scope=True, not both or neither")
        listed = [dict(ANY_SCOPE)] if any_scope else [dict(s) for s in scopes or ()]
        if not listed:
            raise ValueError("scopes is empty: the credential could do nothing")
        role = role.value if isinstance(role, Role) else role
        params: dict[str, Any] = {"principal": principal, "role": role, "scopes": listed}
        if ttl_ms is not None:
            params["ttl_ms"] = ttl_ms
        return CreatedToken.from_dict(self.call("tokens.create", params))

    def rotate_token(self, token_id: str, ttl_ms: int | None = None) -> CreatedToken:
        """Replace a credential's secret, keeping principal and scopes; the old one is
        revoked."""
        params: dict[str, Any] = {"id": token_id}
        if ttl_ms is not None:
            params["ttl_ms"] = ttl_ms
        return CreatedToken.from_dict(self.call("tokens.rotate", params))

    def tokens(self) -> list[TokenInfo]:
        return [TokenInfo.from_dict(t) for t in self.call("tokens.list")]

    def revoke_token(self, token_id: str) -> TokenInfo:
        return TokenInfo.from_dict(self.call("tokens.revoke", {"id": token_id}))

    # -------------------------------------------------------------- approvals

    def request_approval(self, request: Mapping[str, Any], ttl_ms: int | None = None) -> Approval:
        """Ask for approval of a command before submitting it. Only for commands that
        policy holds for approval. Once approved, ``submit(request, approval_id=...)``."""
        params: dict[str, Any] = {"request": dict(request)}
        if ttl_ms is not None:
            params["ttl_ms"] = ttl_ms
        return Approval.from_dict(self.call("approvals.request", params))

    def approval(self, approval_id: str) -> Approval:
        return Approval.from_dict(self.call("approvals.get", {"id": approval_id}))

    def approvals(
        self, statuses: Iterable[ApprovalStatus | str] = (), limit: int | None = None
    ) -> list[Approval]:
        """Requesters see their own; approvers see all."""
        params: dict[str, Any] = {
            "statuses": [s.value if isinstance(s, ApprovalStatus) else s for s in statuses]
        }
        if limit is not None:
            params["limit"] = limit
        return [Approval.from_dict(a) for a in self.call("approvals.list", params)]

    def approve_approval(self, approval_id: str) -> Approval:
        """Approve (an approver who is not the requester). A held command runs."""
        return Approval.from_dict(self.call("approvals.approve", {"id": approval_id}))

    def deny_approval(self, approval_id: str, reason: str | None = None) -> Approval:
        return Approval.from_dict(self.call("approvals.deny", _with_reason(approval_id, reason)))

    def revoke_approval(self, approval_id: str, reason: str | None = None) -> Approval:
        """Withdraw an approval that has not been used."""
        return Approval.from_dict(self.call("approvals.revoke", _with_reason(approval_id, reason)))

    def audit_verify(self) -> AuditVerifyResult:
        return AuditVerifyResult.from_dict(self.call("audit.verify"))

    def audit_tail(self, limit: int | None = None) -> list[AuditEntry]:
        params = {} if limit is None else {"limit": limit}
        return [AuditEntry.from_dict(e) for e in self.call("audit.tail", params)]

    def sim_fault(self, device: str, fault: str) -> str:
        """Inject a simulator fault (``serve --sim`` only), e.g. ``"stale=5000"``."""
        return str(self.call("sim.fault", {"device": device, "fault": fault})["message"])


def _request(
    device: str,
    capability: str,
    operation: str,
    *,
    value: Any = None,
    params: Mapping[str, Any] | None = None,
    idempotency_key: str | None = None,
    correlation_id: str | None = None,
    deadline_ms: int | None = None,
) -> dict[str, Any]:
    req: dict[str, Any] = {
        "target": {"device": device, "capability": capability},
        "operation": operation,
    }
    if operation == "write":
        req["value"] = dump_value(value)
    if params:
        req["params"] = {k: dump_value(v) for k, v in params.items()}
    if idempotency_key is not None:
        req["idempotency_key"] = idempotency_key
    if correlation_id is not None:
        req["correlation_id"] = correlation_id
    if deadline_ms is not None:
        req["deadline_ms"] = deadline_ms
    return req
