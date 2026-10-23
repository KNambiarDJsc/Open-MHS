"""Exceptions raised by the client.

A command that policy or validation refuses is *not* an exception: ``submit`` returns
its receipt with ``outcome == "denied"`` and the reasons attached. Exceptions are for
requests that could not become a command at all (spec/api.md).
"""

from __future__ import annotations

from typing import Any


class OpenMHSError(Exception):
    """Base class for every error this package raises."""


class NotRunningError(OpenMHSError):
    """No runtime file was found: the gateway is not running in that home."""


class GatewayUnreachableError(OpenMHSError):
    """The gateway could not be reached over HTTP."""


class UnauthorizedError(OpenMHSError):
    """The gateway rejected the token (HTTP 401, ``gateway.unauthorized``)."""

    code = "gateway.unauthorized"


class ProtocolError(OpenMHSError):
    """The gateway's answer was not a valid JSON-RPC response."""


class GatewayError(OpenMHSError):
    """The gateway answered with a JSON-RPC error.

    Branch on :attr:`code` (stable, spec/errors.md), never on the message.
    """

    def __init__(
        self,
        code: str,
        message: str,
        rpc_code: int,
        errors: list[Any] | None = None,
        command_id: str | None = None,
    ) -> None:
        super().__init__(f"[{code}] {message}")
        self.code = code
        self.message = message
        self.rpc_code = rpc_code
        #: Detailed :class:`openmhs.ErrorObject` items, when the gateway sent them.
        self.errors = errors or []
        #: For ``command.idempotency.conflict``: the existing command.
        self.command_id = command_id
