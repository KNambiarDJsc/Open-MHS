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

    def submit(self, request: Mapping[str, Any]) -> Receipt:
        """Submit a raw command request (command.schema.json) and wait for its receipt:
        terminal, ``UNKNOWN_OUTCOME`` or ``AWAITING_APPROVAL``."""
        return Receipt.from_dict(self.call("commands.submit", {"request": dict(request)}))

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

    def create_token(self, principal: str, role: Role | str = Role.AGENT) -> CreatedToken:
        role = role.value if isinstance(role, Role) else role
        return CreatedToken.from_dict(
            self.call("tokens.create", {"principal": principal, "role": role})
        )

    def tokens(self) -> list[TokenInfo]:
        return [TokenInfo.from_dict(t) for t in self.call("tokens.list")]

    def revoke_token(self, token_id: str) -> TokenInfo:
        return TokenInfo.from_dict(self.call("tokens.revoke", {"id": token_id}))

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
