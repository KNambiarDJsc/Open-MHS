"""OpenMHS: a Python client for the OpenMHS gateway.

Independent open-source project; not affiliated with Anthropic. Not a safety system:
OpenMHS supervises software commands and does not replace hardware interlocks.
"""

from .client import Client
from .errors import (
    GatewayError,
    GatewayUnreachableError,
    NotRunningError,
    OpenMHSError,
    ProtocolError,
    UnauthorizedError,
)
from .models import (
    AdapterResult,
    AuditEntry,
    AuditVerifyResult,
    CommandState,
    CreatedToken,
    Decision,
    Descriptor,
    ErrorObject,
    GatewayInfo,
    Outcome,
    PolicyRecord,
    PreflightResult,
    Quantity,
    ReadResult,
    Receipt,
    Role,
    RuleMatch,
    StateValue,
    TokenInfo,
    Transition,
    Verification,
)
from .runtime import RuntimeInfo, default_home

__version__ = "0.1.0.dev0"

__all__ = [
    "AdapterResult",
    "AuditEntry",
    "AuditVerifyResult",
    "Client",
    "CommandState",
    "CreatedToken",
    "Decision",
    "Descriptor",
    "ErrorObject",
    "GatewayError",
    "GatewayInfo",
    "GatewayUnreachableError",
    "NotRunningError",
    "OpenMHSError",
    "Outcome",
    "PolicyRecord",
    "PreflightResult",
    "ProtocolError",
    "Quantity",
    "ReadResult",
    "Receipt",
    "Role",
    "RuleMatch",
    "RuntimeInfo",
    "StateValue",
    "TokenInfo",
    "Transition",
    "UnauthorizedError",
    "Verification",
    "default_home",
]
