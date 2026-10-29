"""Export an inspectable experiment report from a running OpenMHS gateway.

    python export_report.py --home <gateway home> [--device lab.bench] [--out report] [--anchor head.json]

Writes ``report.json`` and ``report.md``. It reads only through the gateway's operator API
(commands, audit, descriptors) and changes nothing. The report is a *view of the journal*:
it is exactly as trustworthy as the journal and says so in its header.

What it carries, per command: who asked (principal, credential), what was requested, the
authorization decision, the full lifecycle with timestamps, the device's own reference for the
work (e.g. a LabThings invocation id), the device's answer, the verification evidence (expected,
observed, when, from what source), and any error. For the whole run: the audit chain's
verification, its head (sequence number and hash) as an **anchor**, and the device contract
hashes recorded at registration.

An anchor is what makes tail truncation detectable. A hash chain proves the records that are
there are in order and unaltered; it cannot prove that later records were not removed. Keep
``--anchor`` somewhere the gateway's writer cannot reach, and compare it at the next export.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "python"))
from openmhs import Client  # noqa: E402


def canonical(v) -> str:
    return json.dumps(v, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def _plans(client: Client) -> list[dict]:
    """Plans with the decisions people made on them: who decided what, and when."""
    try:
        plans = client.call("plans.list", {"limit": 200})
    except Exception:
        return []
    keep = ("id", "principal", "state", "reason", "created_at", "updated_at", "decisions")
    out = []
    for p in sorted(plans, key=lambda x: x["created_at"]):
        d = {k: p.get(k) for k in keep}
        d["steps"] = [{k: st.get(k) for k in ("id", "status", "command_id", "command_ids", "attempts") if k in st}
                      for st in p.get("steps", [])]
        out.append(d)
    return out


def _same(receipt: dict, chain: list[tuple[str, str | None]]) -> bool:
    """The receipt's transitions after CREATED, and its final state, as the chain recorded them."""
    mine = [(t["state"], t.get("detail")) for t in receipt["transitions"][1:]]
    return mine == [(to, d) for to, d in chain] and (not chain or chain[-1][0] == receipt["state"])


def build(client: Client, device: str | None = None, anchor: dict | None = None) -> dict:
    verify = client.call("audit.verify", {})
    audit = client.call("audit.tail", {"limit": 100000})
    audit.sort(key=lambda e: e["seq"])
    head = audit[-1] if audit else None
    commands = client.call("commands.list", {"limit": 500})
    commands.sort(key=lambda r: r["created_at"])
    if device:
        commands = [r for r in commands if r["request"]["target"]["device"] == device]
    registered = [e for e in audit if e["kind"] == "device.registered"]
    by_command: dict[str, list] = {}
    for e in audit:
        if e["kind"].startswith("command."):
            by_command.setdefault(e["subject"], []).append(e["seq"])

    # The receipt (a mutable row) against the chained transition records. The chain does not
    # cover the receipt, so this is what notices a receipt edited after the fact.
    chained: dict[str, list[tuple[str, str | None]]] = {}
    for e in audit:
        if e["kind"] == "command.transition":
            d = e["data"] if isinstance(e["data"], dict) else json.loads(e["data"])
            chained.setdefault(e["subject"], []).append((d["to"], d.get("detail")))

    runs = []
    for r in commands:
        ar, ver = r.get("adapter_result") or {}, r.get("verification") or {}
        obs = ver.get("observed") or {}
        pol = r.get("policy") or {}
        created = next((a for a in audit if a["kind"] == "command.created" and a["subject"] == r["command_id"]), None)
        runs.append({
            "command_id": r["command_id"],
            "principal": r["principal"],
            "credential_id": ((created or {}).get("data") or {}).get("origin", {}).get("credential_id"),
            "request": r["request"],
            "authorization": {"decision": pol.get("decision"), "rules": [x.get("rule_id") for x in pol.get("rules", [])],
                              "approved_by": pol.get("approved_by"),
                              "errors": [e["code"] for e in pol.get("errors", [])]},
            "lifecycle": [{"state": t["state"], "at": t["at"], "detail": t.get("detail")} for t in r["transitions"]],
            "final_state": r["state"], "outcome": r["outcome"],
            "device_reference": ar.get("external_ref"),
            "device_result": ar.get("result"), "device_error": (ar.get("error") or {}).get("message"),
            "verification": {"mode": ver.get("mode"), "expected": ver.get("expected"), "matched": ver.get("matched"),
                             "observed": obs.get("value"), "observed_at": obs.get("timestamp"),
                             "observed_source": obs.get("source"), "quality": obs.get("quality"),
                             "reason": ver.get("reason")} if ver else None,
            "audit_seqs": by_command.get(r["command_id"], []),
            "receipt_matches_chain": _same(r, chained.get(r["command_id"], [])),
        })

    report = {
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "caveat": ("A view of the gateway's journal. It is only as trustworthy as the journal; the audit chain "
                   "detects alteration and reordering of the records present, not removal of the newest ones "
                   "(compare the anchor) and not a gateway that lied. It records the device's reports, not the "
                   "physical truth."),
        "audit": {"records": verify["records"], "valid": verify["valid"], "first_invalid": verify.get("first_invalid"),
                  "anchor": {"seq": head["seq"], "hash": head["hash"], "at": head["at"]} if head else None},
        "device_contracts": [{"seq": e["seq"], "device": e["subject"], "descriptor_sha256": (e["data"] or {}).get("descriptor_sha256"),
                              "adapter": (e["data"] or {}).get("adapter"), "at": e["at"]} for e in registered],
        "plans": _plans(client),
        "commands": runs,
        "unresolved": [x["command_id"] for x in runs if x["final_state"] == "UNKNOWN_OUTCOME"],
        "receipts_inconsistent_with_chain": [x["command_id"] for x in runs if not x["receipt_matches_chain"]],
    }
    if anchor:
        a = report["audit"]["anchor"]
        earlier = next((e for e in audit if e["seq"] == anchor["seq"]), None)
        report["audit"]["against_earlier_anchor"] = {
            "earlier": anchor,
            "present": bool(earlier), "hash_matches": bool(earlier and earlier["hash"] == anchor["hash"]),
            "verdict": ("consistent" if earlier and earlier["hash"] == anchor["hash"] else
                        "TRUNCATED OR REWRITTEN: the earlier head is missing or different"),
            "records_since": (a["seq"] - anchor["seq"]) if a else None,
        }
    return report


def to_markdown(rep: dict) -> str:
    a = rep["audit"]
    out = ["# Experiment report", "", f"Generated {rep['generated_at']}.", "", f"> {rep['caveat']}", "",
           "## Audit chain", "",
           f"- records: **{a['records']}**, chain valid: **{a['valid']}**"
           + (f", first invalid: {a['first_invalid']}" if a.get("first_invalid") else ""),
           f"- anchor (keep this somewhere the gateway's writer cannot reach): seq {a['anchor']['seq']}, "
           f"hash `{a['anchor']['hash'][:16]}…`" if a.get("anchor") else "- empty"]
    if "against_earlier_anchor" in a:
        x = a["against_earlier_anchor"]
        out.append(f"- against the earlier anchor (seq {x['earlier']['seq']}): **{x['verdict']}**")
    out += ["", "## Device contracts in force", ""]
    for c in rep["device_contracts"]:
        out.append(f"- seq {c['seq']} `{c['device']}` adapter `{(c['adapter'] or {}).get('id')}` "
                   f"descriptor sha256 `{(c['descriptor_sha256'] or 'not recorded')[:16]}`")
    for p in rep.get("plans", []):
        out += ["", f"## Plan `{p['id'][-12:]}` by `{p['principal']}`: **{p['state']}**", "",
                f"- steps: " + ", ".join(f"{st['id']} {st['status']}" for st in p["steps"])]
        for d in p.get("decisions") or []:
            out.append(f"- decision: {json.dumps(d, default=str)}")
    out += ["", "## Commands", ""]
    for c in rep["commands"]:
        req = c["request"]
        out += [f"### `{c['command_id'][-12:]}` {req['target']['device']} / {req['target']['capability']} → **{c['final_state']}**", "",
                f"- asked by `{c['principal']}` (credential `{c['credential_id']}`); {req['operation']} "
                f"{json.dumps(req.get('value') or req.get('params'))}",
                f"- authorization: {c['authorization']['decision']} by {c['authorization']['rules']}"
                + (f", approved by {c['authorization']['approved_by']}" if c["authorization"]["approved_by"] else "")
                + (f", refused: {c['authorization']['errors']}" if c["authorization"]["errors"] else ""),
                f"- device's own reference: `{c['device_reference']}`" if c["device_reference"] else "- device reference: none recorded",
                "- lifecycle: " + " → ".join(f"{t['state']} ({t['at'][11:23]})" for t in c["lifecycle"])]
        v = c["verification"]
        if v:
            out.append(f"- verification: {v['mode']}; expected {v['expected']}, observed {v['observed']} at {v['observed_at']} "
                       f"({v['observed_source']}), matched {v['matched']}" + (f"; {v['reason']}" if v["reason"] else ""))
        if c["device_error"]:
            out.append(f"- device error: {c['device_error']}")
        if not c["receipt_matches_chain"]:
            out.append("- **the receipt does not match the audit chain's transition records**")
        out.append("")
    if rep["unresolved"]:
        out += ["## Unresolved", "", *[f"- `{x}`" for x in rep["unresolved"]], ""]
    return "\n".join(out)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--home", required=True)
    ap.add_argument("--device")
    ap.add_argument("--out", default="report")
    ap.add_argument("--anchor", help="a file holding an earlier anchor; updated after the export")
    args = ap.parse_args()
    client = Client.from_home(args.home)
    prior = json.loads(Path(args.anchor).read_text()) if args.anchor and Path(args.anchor).exists() else None
    rep = build(client, args.device, prior)
    Path(args.out + ".json").write_text(json.dumps(rep, indent=2, default=str), encoding="utf-8")
    Path(args.out + ".md").write_text(to_markdown(rep), encoding="utf-8")
    if args.anchor and rep["audit"]["anchor"]:
        Path(args.anchor).write_text(json.dumps(rep["audit"]["anchor"]), encoding="utf-8")
    print(f"wrote {args.out}.json and {args.out}.md: {len(rep['commands'])} commands, audit valid={rep['audit']['valid']}")


if __name__ == "__main__":
    main()
