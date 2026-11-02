"""PLR-01..PLR-07 and a latency run: PyLabRobot's simulated liquid handler through OpenMHS.

Each scenario starts a real gateway (`openmhs serve`) in a fresh home with the PyLabRobot
adapter (adapters/python/pylabrobot) as its out-of-process child, drives it with the Python
client, and checks the outcome against two independent records:

- the OpenMHS receipts and audit log (what the gateway says happened), and
- the simulator's own state file, written by the adapter after every completed transfer:
  PyLabRobot's tracker state and its execution counter ``count`` (what the simulator says
  happened). Duplicate or blindly repeated transfers show up there, whatever the gateway says.

Neither is a physical measurement: the backend is PyLabRobot's chatterbox, which models no
liquid.

    $env:OPENMHS_BIN = "...\\openmhs.exe"     # a build of this repository
    <venv-with-pylabrobot>\\python run_scenarios.py [--out results.json] [--only PLR-04]
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
import platform
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from pathlib import Path

REPO = Path(__file__).resolve().parents[4]
sys.path[:0] = [str(REPO / "python"), str(REPO / "research" / "lab-automation" / "shared")]
from openmhs import Client  # noqa: E402
from run_record import finish_report  # noqa: E402

ADAPTER = REPO / "adapters" / "python" / "pylabrobot" / "plr_adapter.py"
DEVICE = "plr.lh-01"
TRANSFER = "liquid.transfer"


def binary() -> Path:
    path = os.environ.get("OPENMHS_BIN")
    if not path:
        raise SystemExit("set OPENMHS_BIN to an openmhs binary built from this repository")
    return Path(path)


def ul(v: float) -> dict:
    return {"value": v, "unit": "uL"}


class Lab:
    """One gateway home with the PyLabRobot adapter, restartable on the same files."""

    def __init__(self, root: Path, *, tips: int = 96, source_ul: dict | None = None):
        self.dir = root
        self.home = root / "home"
        self.home.mkdir(parents=True)
        self.state_file = root / "plr_state.json"
        self.fault_file = root / "plr_fault.json"
        self.manifest = root / "manifest.json"
        self.manifest.write_text(
            json.dumps(
                {
                    "openmhs_manifest": "0.1",
                    "adapter": {"id": "openmhs.py.pylabrobot", "version": "0.1.0"},
                    "runtime": {
                        "command": [sys.executable, str(ADAPTER)],
                        "env": {"PYTHONPATH": str(REPO / "python")},
                        "protocol": ["0.1"],
                    },
                    "capabilities": {
                        "allow": [
                            {"name": "tips.available", "operations": ["read"]},
                            {"name": "transfer.*", "operations": ["read"]},
                            {"name": "source.*", "operations": ["read"]},
                            {"name": "dest.*", "operations": ["read"]},
                            {"name": TRANSFER, "operations": ["invoke"]},
                        ]
                    },
                    "filesystem": {"write": [str(root)]},
                    "config": {
                        "backend": "chatterbox",
                        "state_file": str(self.state_file),
                        "fault_file": str(self.fault_file),
                        "tips": tips,
                        "source_ul": source_ul or {"A1": 200, "A2": 200, "A3": 200},
                    },
                },
                indent=2,
            )
        )
        self.proc: subprocess.Popen | None = None
        self.operator: Client | None = None
        self.start_ms = 0.0

    def start(self) -> None:
        runtime = self.home / "gateway.json"
        runtime.unlink(missing_ok=True)
        t = time.perf_counter()
        self.log = open(self.dir / "gateway.log", "ab")
        self.proc = subprocess.Popen(
            [str(binary()), "--home", str(self.home), "serve", "--port", "0",
             "--adapter-manifest", str(self.manifest), "--allow-unsigned-adapters"],
            stdout=subprocess.DEVNULL,
            stderr=self.log,
        )
        deadline = time.time() + 60
        while time.time() < deadline:
            if runtime.exists():
                try:
                    info = json.loads(runtime.read_text())
                    if info.get("pid") == self.proc.pid:
                        self.operator = Client(info["url"], info["token"])
                        self.start_ms = (time.perf_counter() - t) * 1000
                        return
                except (json.JSONDecodeError, KeyError):
                    pass
            if self.proc.poll() is not None:
                raise RuntimeError(f"gateway exited: see {self.dir / 'gateway.log'}")
            time.sleep(0.05)
        raise RuntimeError("gateway did not start in 60 s")

    def kill(self) -> None:
        """Hard-kill the gateway and its adapter child: no orderly shutdown."""
        if self.proc and self.proc.poll() is None:
            if platform.system() == "Windows":
                subprocess.run(["taskkill", "/T", "/F", "/PID", str(self.proc.pid)],
                               capture_output=True)
            else:
                self.proc.kill()
            self.proc.wait()
        self.log.close()

    def agent(self, name: str = "agent:plr") -> Client:
        for grant in (
            {"id": f"{name}.read", "principal": name, "device": DEVICE, "capability": "*",
             "operations": ["read"], "effect": "allow"},
            {"id": f"{name}.transfer", "principal": name, "device": DEVICE,
             "capability": TRANSFER, "operations": ["invoke"], "effect": "allow"},
        ):
            with contextlib.suppress(Exception):  # already granted after a restart
                self.operator.add_grant(grant)
        token = self.operator.create_token(name, any_scope=True)
        return Client(self.operator.url if hasattr(self.operator, "url") else self.url(), token.token)

    def url(self) -> str:
        return json.loads((self.home / "gateway.json").read_text())["url"]

    def sim(self) -> dict:
        """The simulator's own record (PyLabRobot tracker state and its counter)."""
        state = json.loads(self.state_file.read_text())
        deck = state["deck"]
        vol = lambda plate, well: deck[f"{plate}_well_{well}"]["volume"]
        return {
            "count": state["count"],
            "last_id": state["last_id"],
            "source": {w: vol("source", w) for w in ("A1", "A2", "A3")},
            "dest": {w: vol("dest", w) for w in ("B1", "B2", "B3")},
            "tips": sum(1 for k, v in deck.items() if k.startswith("tips_tipspot") and v.get("tip")),
        }

    def fault(self, name: str) -> None:
        self.fault_file.write_text(json.dumps({"next_transfer": name}))


def transfer(client: Client, transfer_id: str, src="A1", dst="B1", volume=50.0, key=None, timeout=30) -> dict:
    request = {
        "target": {"device": DEVICE, "capability": TRANSFER},
        "operation": "invoke",
        "params": {"transfer_id": transfer_id, "source": src, "dest": dst, "volume": ul(volume)},
    }
    if key:
        request["idempotency_key"] = key
    return client.call("commands.submit", {"request": request}, timeout=timeout)


def states(receipt: dict) -> list[str]:
    return [t["state"] for t in receipt["transitions"]]


def codes(receipt: dict) -> list[str]:
    return [e["code"] for e in receipt["policy"].get("errors", [])]


def reached_device(receipt: dict) -> bool:
    return "DISPATCHED" in states(receipt)


def summary(r: dict) -> dict:
    return {"command_id": r["command_id"], "state": r["state"], "transitions": states(r),
            "errors": codes(r), "verification": r.get("verification")}


class Check:
    def __init__(self) -> None:
        self.items: list[dict] = []

    def __call__(self, what: str, ok: bool, observed=None) -> None:
        self.items.append({"check": what, "ok": bool(ok), "observed": observed})

    @property
    def passed(self) -> bool:
        return all(i["ok"] for i in self.items)


# ------------------------------------------------------------------ scenarios


def plr01(lab: Lab, check: Check) -> dict:
    agent = lab.agent()
    before = lab.sim()
    r = transfer(agent, "t-plr01")
    after = lab.sim()
    value = lambda cap: agent.read(DEVICE, cap).to_dict()["state"]["value"]
    reported = {
        "source_a1": value("source.a1.volume"),
        "dest_b1": value("dest.b1.volume"),
        "tips": value("tips.available"),
    }
    check("receipt VERIFIED", r["state"] == "VERIFIED", r["state"])
    check("simulator executed exactly once", after["count"] - before["count"] == 1, after["count"])
    check("tracker moved 50 uL A1 -> B1",
          (before["source"]["A1"] - after["source"]["A1"], after["dest"]["B1"]) == (50.0, 50.0),
          (after["source"]["A1"], after["dest"]["B1"]))
    check("one tip used", before["tips"] - after["tips"] == 1, after["tips"])
    check("reads agree with the simulator's state",
          (reported["source_a1"]["value"], reported["dest_b1"]["value"], reported["tips"])
          == (after["source"]["A1"], after["dest"]["B1"], after["tips"]), reported)
    return {"receipt": summary(r), "simulator_before": before, "simulator_after": after}


def plr02(lab: Lab, check: Check) -> dict:
    agent = lab.agent()
    before = lab.sim()
    cases = {
        "volume 500 uL (above 200)": transfer(agent, "t-big", volume=500.0),
        "volume 0.5 uL (below 1)": transfer(agent, "t-small", volume=0.5),
        "unknown source well C7": transfer(agent, "t-well", src="C7"),
        "destination used as source": transfer(agent, "t-swap", src="B1", dst="A1"),
    }
    after = lab.sim()
    for name, r in cases.items():
        check(f"{name}: DENIED", r["state"] == "DENIED", (r["state"], codes(r)))
        check(f"{name}: never dispatched", not reached_device(r), states(r))
    check("simulator untouched", after == before, after["count"])
    return {"receipts": {k: summary(v) for k, v in cases.items()}}


def plr03(lab: Lab, check: Check) -> dict:
    agent = lab.agent()
    first = transfer(agent, "t-last-tip")
    before = lab.sim()
    second = transfer(agent, "t-no-tip")
    after = lab.sim()
    check("the rack's only tip is used", first["state"] == "VERIFIED" and before["tips"] == 0,
          (first["state"], before["tips"]))
    check("no tip: DENIED by OpenMHS's precondition", second["state"] == "DENIED"
          and any("precondition" in c for c in codes(second)), (second["state"], codes(second)))
    check("never dispatched", not reached_device(second), states(second))
    check("simulator untouched", after == before)
    native = native_missing_tip()
    check("PyLabRobot itself also refuses a pickup from an empty spot", native["refused"], native)
    return {"with_tip": summary(first), "without_tip": summary(second), "native": native}


def native_missing_tip() -> dict:
    """Without OpenMHS: what PyLabRobot does when asked to pick up a tip that is not there."""
    import asyncio

    from pylabrobot.liquid_handling import LiquidHandler
    from pylabrobot.liquid_handling.backends.chatterbox import LiquidHandlerChatterboxBackend
    from pylabrobot.resources import TIP_CAR_480_A00, hamilton_96_tiprack_300uL, set_tip_tracking
    from pylabrobot.resources.hamilton import STARLetDeck

    async def run() -> dict:
        set_tip_tracking(True)
        deck = STARLetDeck()
        lh = LiquidHandler(backend=LiquidHandlerChatterboxBackend(num_channels=1), deck=deck)
        tips = hamilton_96_tiprack_300uL(name="tips", with_tips=False)
        car = TIP_CAR_480_A00(name="c")
        car[0] = tips
        deck.assign_child_resource(car, rails=1)
        with contextlib.redirect_stdout(io.StringIO()):
            await lh.setup()
            try:
                await lh.pick_up_tips(tips["A1"])
                return {"refused": False}
            except Exception as e:
                return {"refused": True, "error": f"{type(e).__module__}.{type(e).__name__}: {e}"}

    return asyncio.run(run())


def plr04(lab: Lab, check: Check) -> dict:
    agent = lab.agent()
    transfer(agent, "t-warmup", dst="B2", volume=10.0)
    before = lab.sim()
    lab.fault("apply_then_hang")
    t = time.perf_counter()
    lost = transfer(agent, "t-lost")
    waited_ms = (time.perf_counter() - t) * 1000
    mid = lab.sim()
    check("the simulator applied the transfer", mid["count"] - before["count"] == 1
          and mid["last_id"] == "t-lost", mid)
    check("receipt is UNKNOWN_OUTCOME, not success", lost["state"] == "UNKNOWN_OUTCOME",
          (lost["state"], states(lost)))
    held = transfer(agent, "t-while-unknown", dst="B3", volume=10.0)
    check("next transfer on the capability is held, not sent", held["state"] == "DENIED"
          and not reached_device(held), (held["state"], codes(held)))
    check("no blind repeat", lab.sim()["count"] == mid["count"])
    t = time.perf_counter()
    reconciled = lab.operator.call("commands.reconcile", {"command_id": lost["command_id"]})
    reconcile_ms = (time.perf_counter() - t) * 1000
    check("reconciled from the simulator's state: RECONCILED_APPLIED",
          reconciled["state"] == "RECONCILED_APPLIED", reconciled["state"])
    after_reconcile = transfer(agent, "t-after", dst="B3", volume=10.0)
    check("capability free again after reconciliation", after_reconcile["state"] == "VERIFIED",
          after_reconcile["state"])
    final = lab.sim()
    check("simulator executed t-lost exactly once", final["count"] - before["count"] == 2, final["count"])
    return {"lost": summary(lost), "held": summary(held), "reconciled": summary(reconciled),
            "after": summary(after_reconcile), "waited_ms": round(waited_ms),
            "reconcile_ms": round(reconcile_ms)}


def plr05(lab: Lab, check: Check) -> dict:
    agent = lab.agent()
    before = lab.sim()
    first = transfer(agent, "t-dup", key="dup-1")
    again = transfer(agent, "t-dup", key="dup-1")
    mid = lab.sim()
    check("first VERIFIED", first["state"] == "VERIFIED", first["state"])
    check("same key returns the same command", again["command_id"] == first["command_id"],
          (first["command_id"], again["command_id"]))
    check("executed once", mid["count"] - before["count"] == 1, mid["count"])
    # Same transfer_id under a new key: a different command, which must not double the
    # transfer or verify against the earlier one.
    reused = transfer(agent, "t-dup", key="dup-2")
    after = lab.sim()
    check("a reused transfer_id is refused by the simulator adapter, nothing applied",
          reused["state"] == "REJECTED" and after == mid, (reused["state"], after["count"]))
    no_key = [transfer(agent, f"t-nokey-{i}", dst="B2", volume=5.0) for i in range(2)]
    check("without a key, two submissions are two transfers (documented)",
          lab.sim()["count"] - after["count"] == 2)
    return {"first": summary(first), "again": summary(again), "reused_id": summary(reused),
            "no_key": [summary(r) for r in no_key]}


def plr06(lab: Lab, check: Check) -> dict:
    a, b = lab.agent("agent:a"), lab.agent("agent:b")
    before = lab.sim()
    results: dict = {}

    def go(name: str, client: Client) -> None:
        results[name] = transfer(client, f"t-{name}", src="A1", dst="B1" if name == "a" else "B2",
                                 volume=80.0)

    threads = [threading.Thread(target=go, args=(n, c)) for n, c in (("a", a), ("b", b))]
    for th in threads:
        th.start()
    for th in threads:
        th.join()
    after = lab.sim()
    outcome = sorted(r["state"] for r in results.values())
    check("one VERIFIED, one REJECTED (only 100 uL for two 80 uL transfers)",
          outcome == ["REJECTED", "VERIFIED"], outcome)
    loser = next((r for r in results.values() if r["state"] != "VERIFIED"), {})
    reason = json.dumps(loser.get("adapter_result") or {})
    check("the refusal came from PyLabRobot's volume tracker", "PyLabRobot refused" in reason, reason)
    check("source never went negative; 80 uL moved once",
          (after["source"]["A1"], after["count"] - before["count"]) == (20.0, 1),
          (after["source"]["A1"], after["count"]))
    windows = []
    for r in results.values():
        ts = {t["state"]: t["at"] for t in r["transitions"]}
        if "DISPATCHED" in ts:
            windows.append(ts["DISPATCHED"])
    return {"a": summary(results["a"]), "b": summary(results["b"]), "dispatch_times": windows,
            "rejected_reason": reason}


def plr07(lab: Lab, check: Check) -> dict:
    out = {}
    for variant, fault, applied in (("applied", "apply_then_hang", True),
                                    ("not_applied", "hang_before_apply", False)):
        agent = lab.agent()
        before = lab.sim()
        lab.fault(fault)
        holder: dict = {}

        def go() -> None:
            try:
                holder["r"] = transfer(agent, f"t-crash-{variant}", dst="B2", volume=10.0, timeout=60)
            except Exception as e:  # the gateway dies under the call
                holder["error"] = type(e).__name__

        th = threading.Thread(target=go, daemon=True)
        th.start()
        deadline = time.time() + 20
        while time.time() < deadline:
            cmds = lab.operator.call("commands.list", {"limit": 5})
            mine = [c for c in cmds if c["request"]["params"]["transfer_id"] == f"t-crash-{variant}"]
            if mine and mine[0]["state"] in ("DISPATCHED", "ACCEPTED"):
                break
            time.sleep(0.05)
        if applied:
            while lab.sim()["last_id"] != f"t-crash-{variant}" and time.time() < deadline:
                time.sleep(0.05)
        else:
            time.sleep(0.3)
        command_id = mine[0]["command_id"]
        lab.kill()
        th.join(5)
        mid = lab.sim()
        t = time.perf_counter()
        lab.start()
        restart_ms = (time.perf_counter() - t) * 1000
        recovered = lab.operator.call("commands.get", {"command_id": command_id})
        check(f"{variant}: after restart the command is UNKNOWN_OUTCOME, not resent",
              recovered["state"] == "UNKNOWN_OUTCOME" and lab.sim()["count"] == mid["count"],
              (recovered["state"], lab.sim()["count"]))
        reconciled = lab.operator.call("commands.reconcile", {"command_id": command_id})
        expect = "RECONCILED_APPLIED" if applied else "RECONCILED_NOT_APPLIED"
        check(f"{variant}: reconciliation reads the simulator: {expect}",
              reconciled["state"] == expect, reconciled["state"])
        final = lab.sim()
        check(f"{variant}: simulator count {'+1' if applied else '+0'}, never repeated",
              final["count"] - before["count"] == (1 if applied else 0), final["count"])
        out[variant] = {"recovered": summary(recovered), "reconciled": summary(reconciled),
                        "client": holder.get("error") or holder.get("r", {}).get("state"),
                        "restart_ms": round(restart_ms)}
    audit = lab.operator.audit_verify().to_dict()
    check("audit chain intact across both crashes", audit["valid"], audit)
    return out


def perf(lab: Lab, check: Check, n: int = 30) -> dict:
    agent = lab.agent()
    latencies = []
    for i in range(n):
        t = time.perf_counter()
        r = transfer(agent, f"t-perf-{i}", src="A2", dst="B3", volume=1.0)
        latencies.append((time.perf_counter() - t) * 1000)
        assert r["state"] == "VERIFIED", r["state"]
    native = native_latency(n)
    lat = sorted(latencies)
    journal = lab.home / "journal.sqlite"
    result = {
        "n": n,
        "openmhs_ms": {"median": round(statistics.median(lat), 1), "p95": round(lat[int(0.95 * (n - 1))], 1),
                       "max": round(lat[-1], 1)},
        "native_ms": native,
        "gateway_start_ms": round(lab.start_ms),
        "journal_bytes": journal.stat().st_size,
        "audit_records": len(lab.operator.audit_tail(500)),
    }
    check(f"{n} governed transfers all VERIFIED", True, result["openmhs_ms"])
    return result


def native_latency(n: int) -> dict:
    """The same transfer in-process with PyLabRobot alone (baseline A)."""
    import asyncio

    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "native_baseline"))

    async def run() -> list[float]:
        from pylabrobot.liquid_handling import LiquidHandler
        from pylabrobot.liquid_handling.backends.chatterbox import LiquidHandlerChatterboxBackend
        from pylabrobot.resources import (PLT_CAR_L5AC_A00, TIP_CAR_480_A00, cor_96_wellplate_360uL_Fb,
                                          hamilton_96_tiprack_300uL)
        from pylabrobot.resources.hamilton import STARLetDeck

        deck = STARLetDeck()
        lh = LiquidHandler(backend=LiquidHandlerChatterboxBackend(num_channels=1), deck=deck)
        tips = hamilton_96_tiprack_300uL(name="tips")
        src, dst = cor_96_wellplate_360uL_Fb(name="source"), cor_96_wellplate_360uL_Fb(name="dest")
        tc, pc = TIP_CAR_480_A00(name="tc"), PLT_CAR_L5AC_A00(name="pc")
        tc[0], pc[0], pc[1] = tips, src, dst
        deck.assign_child_resource(tc, rails=1)
        deck.assign_child_resource(pc, rails=10)
        out = []
        with contextlib.redirect_stdout(io.StringIO()):
            await lh.setup()
            src.get_well("A2").tracker.set_volume(200)
            for i in range(n):
                t = time.perf_counter()
                await lh.pick_up_tips(tips[i])
                await lh.aspirate([src.get_well("A2")], vols=[1.0])
                await lh.dispense([dst.get_well("B3")], vols=[1.0])
                await lh.discard_tips()
                out.append((time.perf_counter() - t) * 1000)
        return out

    lat = sorted(asyncio.run(run()))
    return {"median": round(statistics.median(lat), 2), "p95": round(lat[int(0.95 * (n - 1))], 2),
            "max": round(lat[-1], 2)}


SCENARIOS = {
    "PLR-01": ("normal transfer A1 -> B1, 50 uL", plr01, {}),
    "PLR-02": ("invalid volume and wells are denied before dispatch", plr02, {}),
    "PLR-03": ("missing tip", plr03, {"tips": 1}),
    "PLR-04": ("applied, acknowledgment lost", plr04, {}),
    "PLR-05": ("duplicate request", plr05, {}),
    "PLR-06": ("two agents compete for one source well", plr06, {"source_ul": {"A1": 100, "A2": 200, "A3": 200}}),
    "PLR-07": ("gateway killed mid-transfer, restarted, reconciled", plr07, {}),
    "PERF": ("latency, governed vs native", perf, {}),
}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="results.json")
    ap.add_argument("--only", nargs="*")
    ap.add_argument("--work", default=None, help="where to keep each run's gateway home")
    args = ap.parse_args()
    import pylabrobot

    work = Path(args.work or tempfile.mkdtemp(prefix="plr-runs-"))
    report = {
        "pylabrobot": pylabrobot.__version__,
        "started": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "scenarios": {},
    }
    for sid, (title, fn, cfg) in SCENARIOS.items():
        if args.only and sid not in args.only:
            continue
        lab = Lab(work / f"{sid}-{uuid.uuid4().hex[:6]}", **cfg)
        check = Check()
        t = time.perf_counter()
        try:
            lab.start()
            detail = fn(lab, check)
            audit = lab.operator.audit_verify().to_dict()
            check("audit chain verifies", audit["valid"], audit["records"])
        except Exception as e:  # report, do not hide
            detail = {"exception": f"{type(e).__name__}: {e}"}
            check("scenario ran to the end", False, detail["exception"])
        finally:
            with contextlib.suppress(Exception):
                lab.kill()
        report["scenarios"][sid] = {
            "title": title,
            "passed": check.passed,
            "seconds": round(time.perf_counter() - t, 1),
            "checks": check.items,
            "detail": detail,
            "run_dir": str(lab.dir),
        }
        mark = "PASS" if check.passed else "FAIL"
        print(f"{mark} {sid} {title} ({report['scenarios'][sid]['seconds']} s)")
        for item in check.items:
            if not item["ok"]:
                print(f"     failed: {item['check']}: {item['observed']}")
    print(f"wrote {finish_report(report, args.out)}")


if __name__ == "__main__":
    main()
