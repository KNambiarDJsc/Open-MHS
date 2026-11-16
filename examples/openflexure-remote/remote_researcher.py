"""Simulator-only, read-only external-researcher access to an OpenFlexure stage.

Uses the existing OpenMHS gateway. The host issues a short-lived, narrowly scoped
credential. An external researcher connects using only a gateway URL and that
credential: no lab-side operator token, home directory, or direct instrument API.

This is NOT an Internet-facing service. Do not expose the gateway, the OpenFlexure
API, or an MCP server publicly. For a real remote connection use a separately
reviewed secure transport, network isolation, and an independent access assessment.

Example:
  python remote_researcher.py issue --home ~/.openmhs-openflexure --session demo1
  OPENMHS_REMOTE_TOKEN=... python remote_researcher.py observe --url http://127.0.0.1:PORT
  python remote_researcher.py revoke --home ~/.openmhs-openflexure --token-id ID
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python"))
from openmhs import Client  # noqa: E402

DEVICE = "ofm.stage"
CAPABILITIES = ("position.x", "position.y", "position.z")
MAX_TTL_MINUTES = 60


def principal_for(session: str) -> str:
    if not session or len(session) > 64 or not all(c.isascii() and (c.isalnum() or c in "_-") for c in session):
        raise ValueError("session must contain 1–64 ASCII letters, digits, '_' or '-'")
    return "agent:remote-researcher:" + session


def issue(home: str, session: str, minutes: int) -> None:
    if not 1 <= minutes <= MAX_TTL_MINUTES:
        raise ValueError("session must last between 1 and 60 minutes")
    operator = Client.from_home(home)
    principal = principal_for(session)
    for axis in "xyz":
        operator.add_grant({
            "id": f"remote-{session}-read-{axis}",
            "principal": principal,
            "device": DEVICE,
            "capability": f"position.{axis}",
            "operations": ["read"],
            "effect": "allow",
        })
    token = operator.create_token(
        principal,
        scopes=[{"device": DEVICE, "capability": c, "operations": ["read"]} for c in CAPABILITIES],
        ttl_ms=minutes * 60_000,
    )
    # Intentionally never write the secret to a config, log or shared directory.
    # Terminal output is sensitive: share only over an independently secured channel.
    print(json.dumps({"session": session, "principal": principal, "token_id": token.id,
                      "expires_in_minutes": minutes}))
    print("SECRET TOKEN (shown once; treat as a password):", token.token)
    print("Host: revoke with 'revoke --token-id', and revoke session grants separately if necessary.")


def observe(url: str) -> None:
    token = os.environ.get("OPENMHS_REMOTE_TOKEN")
    if not token:
        raise ValueError("set OPENMHS_REMOTE_TOKEN in your environment")
    if not url.startswith("http://127.0.0.1:") and not url.startswith("http://localhost:"):
        raise ValueError("this PoC only accepts loopback URLs; no public network exposure")
    researcher = Client(url, token, timeout=10)
    result = {}
    for capability in CAPABILITIES:
        reading = researcher.read(DEVICE, capability)
        # Do not represent missing, stale, or unusable readings as valid measurements.
        result[capability] = {"usable": reading.usable, "state": reading.state.to_dict()}
    print(json.dumps({"device": DEVICE, "readings": result}, indent=2, default=str))


def revoke(home: str, token_id: str) -> None:
    operator = Client.from_home(home)
    result = operator.revoke_token(token_id)
    print(f"Revocation requested for token {token_id}: {result}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="command", required=True)
    a = sub.add_parser("issue", help="host: create one scoped researcher credential")
    a.add_argument("--home", required=True)
    a.add_argument("--session", required=True)
    a.add_argument("--minutes", type=int, default=15)
    b = sub.add_parser("observe", help="researcher: read stage state via gateway")
    b.add_argument("--url", required=True)
    c = sub.add_parser("revoke", help="host: revoke a credential")
    c.add_argument("--home", required=True)
    c.add_argument("--token-id", required=True)
    args = ap.parse_args()
    try:
        if args.command == "issue":
            issue(args.home, args.session, args.minutes)
        elif args.command == "observe":
            observe(args.url)
        else:
            revoke(args.home, args.token_id)
        return 0
    except Exception as exc:
        print(f"remote session failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
