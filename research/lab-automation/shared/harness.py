"""Shared experiment harness: a real `openmhs serve` with one out-of-process adapter.

Each experiment builds a manifest for its adapter and runs scenarios against `Lab`. A lab is
one gateway home that can be hard-killed and restarted on the same files. Checks are
recorded, never asserted, so a run always ends with a complete report.

    from harness import Lab, Check, run_all
"""

from __future__ import annotations

import contextlib
import json
import os
import platform
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path
from typing import Any, Callable

REPO = Path(__file__).resolve().parents[3]
sys.path[:0] = [str(REPO / "python"), str(Path(__file__).resolve().parent)]
from openmhs import Client  # noqa: E402
from run_record import finish_report  # noqa: E402


def binary() -> Path:
    path = os.environ.get("OPENMHS_BIN")
    if not path:
        raise SystemExit("set OPENMHS_BIN to an openmhs binary built from this repository")
    return Path(path)


class Lab:
    """One gateway home with one adapter, restartable on the same files."""

    def __init__(self, root: Path, manifest: dict):
        self.dir = root
        self.home = root / "home"
        self.home.mkdir(parents=True)
        self.manifest = root / "manifest.json"
        self.manifest.write_text(json.dumps(manifest, indent=2))
        self.proc: subprocess.Popen | None = None
        self.operator: Client | None = None
        self.start_ms = 0.0
        #: Reuse the first port after a restart, as a deployed gateway has a fixed one.
        self.sticky_port = False
        self._port = 0

    def start(self, timeout_s: float = 90) -> None:
        runtime = self.home / "gateway.json"
        runtime.unlink(missing_ok=True)
        t = time.perf_counter()
        self.log = open(self.dir / "gateway.log", "ab")
        self.proc = subprocess.Popen(
            [str(binary()), "--home", str(self.home), "serve", "--port", str(self._port),
             "--adapter-manifest", str(self.manifest), "--allow-unsigned-adapters"],
            stdout=subprocess.DEVNULL, stderr=self.log)
        deadline = time.time() + timeout_s
        while time.time() < deadline:
            if runtime.exists():
                with contextlib.suppress(json.JSONDecodeError, KeyError):
                    info = json.loads(runtime.read_text())
                    if info.get("pid") == self.proc.pid:
                        self.url = info["url"]
                        if self.sticky_port:
                            self._port = int(info["url"].rsplit(":", 1)[1])
                        self.operator = Client(info["url"], info["token"])
                        self.start_ms = (time.perf_counter() - t) * 1000
                        return
            if self.proc.poll() is not None:
                raise RuntimeError(f"gateway exited: see {self.dir / 'gateway.log'}")
            time.sleep(0.05)
        raise RuntimeError(f"gateway did not start in {timeout_s} s")

    def kill(self) -> None:
        """Hard-kill the gateway and its adapter child: no orderly shutdown."""
        if self.proc and self.proc.poll() is None:
            if platform.system() == "Windows":
                subprocess.run(["taskkill", "/T", "/F", "/PID", str(self.proc.pid)], capture_output=True)
            else:
                self.proc.kill()
            self.proc.wait()
        with contextlib.suppress(Exception):
            self.log.close()

    def agent(self, name: str, grants: list[dict]) -> Client:
        for grant in grants:
            with contextlib.suppress(Exception):  # already granted after a restart
                # Grant ids are global: prefix them, or a second agent's grants collide.
                self.operator.add_grant({**grant, "principal": name, "id": f"{name}.{grant['id']}"})
        return Client(self.url, self.operator.create_token(name, any_scope=True).token)

    def export_evidence(self, out: Path) -> dict:
        """`openmhs audit export`, then `audit verify-file` on it: the offline check."""
        checkpoint = out.with_suffix(".checkpoint.json")
        for p in (out, checkpoint):
            p.unlink(missing_ok=True)
        subprocess.run([str(binary()), "--home", str(self.home), "audit", "export", "--out", str(out),
                        "--checkpoint-out", str(checkpoint)], check=True, capture_output=True)
        verify = subprocess.run([str(binary()), "--json", "--home", str(self.dir / "nowhere"), "audit",
                                 "verify-file", str(out), "--checkpoint", str(checkpoint)],
                                capture_output=True, text=True)
        return {"exit": verify.returncode, **json.loads(verify.stdout)}


def states(r: dict) -> list[str]:
    return [t["state"] for t in r["transitions"]]


def codes(r: dict) -> list[str]:
    return [e["code"] for e in r["policy"].get("errors", [])]


def reached_device(r: dict) -> bool:
    return "DISPATCHED" in states(r)


def summary(r: dict) -> dict:
    return {"command_id": r["command_id"], "state": r["state"], "transitions": states(r),
            "errors": codes(r), "verification": r.get("verification"),
            "adapter_result": r.get("adapter_result")}


class Check:
    def __init__(self) -> None:
        self.items: list[dict] = []

    def __call__(self, what: str, ok: Any, observed: Any = None) -> None:
        self.items.append({"check": what, "ok": bool(ok), "observed": observed})

    @property
    def passed(self) -> bool:
        return all(i["ok"] for i in self.items)


def run_all(scenarios: dict[str, tuple[str, Callable, Callable[[Path], Lab]]], meta: dict,
            out: str, only: list[str] | None, work: str | None) -> dict:
    """Run each scenario on a fresh lab; write and return the report."""
    root = Path(work or tempfile.mkdtemp(prefix="lab-runs-"))
    report = {
        "started": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        **meta,
        "scenarios": {},
    }
    for sid, (title, fn, make) in scenarios.items():
        if only and sid not in only:
            continue
        lab = make(root / f"{sid}-{uuid.uuid4().hex[:6]}")
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
        report["scenarios"][sid] = {"title": title, "passed": check.passed,
                                    "seconds": round(time.perf_counter() - t, 1),
                                    "checks": check.items, "detail": detail, "run_dir": str(lab.dir)}
        print(f"{'PASS' if check.passed else 'FAIL'} {sid} {title} ({report['scenarios'][sid]['seconds']} s)")
        for item in check.items:
            if not item["ok"]:
                print(f"     failed: {item['check']}: {item['observed']}")
    print(f"wrote {finish_report(report, out)}")
    return report
