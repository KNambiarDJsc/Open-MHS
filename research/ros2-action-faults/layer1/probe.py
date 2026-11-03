"""Layer 1a: probe the premises of the Layer 0 model against a real rclpy action server.

Run inside the ROS 2 Jazzy environment (see README.md):
    python3 probe.py --n 20 --out results/layer1a

Each scenario is repeated n times and every observation is recorded as a JSON line. Counts of
effect events come only from the server's independent ledger file. This is a characterisation of
one RMW (the Jazzy default, Fast DDS) on one machine; it is not a fault-rate study.

Isolation: every trial uses its own action name, so entities left behind by a SIGKILLed server in an
earlier trial cannot interfere. Within a trial the restarted server keeps the same name, as a real
restart would. A resend can go through the original client or through a fresh client object (which
stands for a restarted client process); the two are recorded separately because rclpy keeps its own
table of goal ids.
"""

from __future__ import annotations

import argparse
import inspect
import json
import os
import platform
import signal
import subprocess
import sys
import threading
import time

import rclpy
from example_interfaces.action import Fibonacci
from rclpy.action import ActionClient
from rclpy.executors import MultiThreadedExecutor
from unique_identifier_msgs.msg import UUID

HERE = os.path.dirname(os.path.abspath(__file__))
STATUS = {0: "UNKNOWN", 1: "ACCEPTED", 2: "EXECUTING", 3: "CANCELING", 4: "SUCCEEDED",
          5: "CANCELED", 6: "ABORTED"}
DURATION = 3.0
EFFECT_FRAC = 0.5


class Rig:
    def __init__(self, workdir: str) -> None:
        self.workdir = workdir
        self.ledger = os.path.join(workdir, "ledger.jsonl")
        self.events = os.path.join(workdir, "events.jsonl")
        self.proc: subprocess.Popen | None = None
        self.action = "dose"
        self.client_errors: list[str] = []
        self._running = True
        rclpy.init()
        self.node = rclpy.create_node("probe_client")
        self.ex = MultiThreadedExecutor(num_threads=4)
        self.ex.add_node(self.node)
        self.clients: list[ActionClient] = []
        threading.Thread(target=self._spin, daemon=True).start()

    def _spin(self) -> None:
        # An exception raised inside a callback ends ex.spin(); record it and keep spinning.
        while self._running:
            try:
                self.ex.spin()
            except Exception as exc:  # noqa: BLE001
                self.client_errors.append(f"{type(exc).__name__}: {str(exc)[:160]}")
                time.sleep(0.05)

    # -- per-trial setup ------------------------------------------------------------------
    def new_trial(self, order: int) -> None:
        self.action = f"dose_{order}"
        self.client_errors.clear()
        self.ac = ActionClient(self.node, Fibonacci, self.action)
        self.ac2 = ActionClient(self.node, Fibonacci, self.action)  # a "restarted client process"
        self.clients = [self.ac, self.ac2]

    def end_trial(self) -> None:
        self.stop()
        for c in self.clients:
            c.destroy()
        self.clients = []

    # -- server lifecycle ----------------------------------------------------------------
    def _events_seen(self) -> int:
        if not os.path.exists(self.events):
            return 0
        with open(self.events, encoding="utf-8") as fh:
            return sum(1 for line in fh if '"server_up"' in line)

    def start_server(self, result_timeout: int) -> None:
        before = self._events_seen()
        self.proc = subprocess.Popen(
            [sys.executable, os.path.join(HERE, "server.py"), "--ledger", self.ledger,
             "--events", self.events, "--duration", str(DURATION), "--action-name", self.action,
             "--effect-frac", str(EFFECT_FRAC), "--result-timeout", str(result_timeout)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline = time.time() + 30.0
        while self._events_seen() <= before:
            if time.time() > deadline:
                raise RuntimeError("action server process did not start")
            time.sleep(0.1)
        if not self.ac.wait_for_server(timeout_sec=20.0):
            raise RuntimeError("action server did not come up")

    def kill_server(self) -> None:
        """The fault: SIGKILL, no cleanup, no goodbye to DDS."""
        if self.proc is not None:
            self.proc.kill()
            self.proc.wait()
            self.proc = None

    def stop(self) -> None:
        if self.proc is not None:
            self.proc.send_signal(signal.SIGINT)
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
            self.proc = None

    # -- client helpers ------------------------------------------------------------------
    @staticmethod
    def uuid_from(seed: int) -> UUID:
        return UUID(uuid=list(seed.to_bytes(16, "big")))

    def send(self, client: ActionClient, order: int, uuid: UUID, timeout: float = 8.0):
        """Returns (response, handle): response in 'accepted', 'rejected', 'timeout'."""
        done = threading.Event()
        box: dict = {}
        goal = Fibonacci.Goal()
        goal.order = order
        fut = client.send_goal_async(goal, goal_uuid=uuid)

        def cb(f):
            box["handle"] = f.result()
            done.set()

        fut.add_done_callback(cb)
        if not done.wait(timeout):
            return "timeout", None
        h = box["handle"]
        return ("accepted" if h.accepted else "rejected"), h

    def result(self, handle, timeout: float = 8.0) -> str:
        """Status name, or 'timeout' when no response arrives."""
        done = threading.Event()
        box: dict = {}
        fut = handle.get_result_async()

        def cb(f):
            box["r"] = f.result()
            done.set()

        fut.add_done_callback(cb)
        if not done.wait(timeout):
            return "timeout"
        return STATUS.get(box["r"].status, str(box["r"].status))

    def timeline(self, order: int, t0: float) -> list:
        """Server-side events for one trial, as [event, seconds since the trial started]."""
        out = []
        if os.path.exists(self.events):
            with open(self.events, encoding="utf-8") as fh:
                for line in fh:
                    e = json.loads(line)
                    if e.get("order") == order or (e["event"] == "server_up" and e["t"] >= t0):
                        out.append([e["event"], round(e["t"] - t0, 2)])
        return out

    def effects(self, order: int) -> int:
        if not os.path.exists(self.ledger):
            return 0
        with open(self.ledger, encoding="utf-8") as fh:
            return sum(1 for line in fh if json.loads(line)["order"] == order)


def scenario_dup_held(rig: Rig, order: int, seed: int) -> dict:
    rig.start_server(result_timeout=10)
    u = rig.uuid_from(seed)
    r1, h1 = rig.send(rig.ac, order, u)
    time.sleep(0.5)
    r2, _ = rig.send(rig.ac, order, u)  # same goal id while the goal is still held
    status = rig.result(h1) if h1 else "n/a"
    time.sleep(0.3)
    return {"first": r1, "second_same_uuid": r2, "first_result": status,
            "effects": rig.effects(order)}


def scenario_expiry(rig: Rig, order: int, seed: int, result_timeout: int = 10) -> dict:
    rig.start_server(result_timeout=result_timeout)
    u = rig.uuid_from(seed)
    r1, h1 = rig.send(rig.ac, order, u)
    first_status = rig.result(h1) if h1 else "n/a"
    time.sleep(result_timeout + 1.5)  # let the retained result expire
    late_status = rig.result(h1) if h1 else "n/a"  # ask again for the same goal id
    if not rig.ac2.wait_for_server(timeout_sec=10.0):
        pass
    r2, h2 = rig.send(rig.ac2, order, u)  # resend the same goal id from a fresh client
    second_status = rig.result(h2) if h2 else "n/a"
    return {"first": r1, "first_status": first_status, "late_status_after_expiry": late_status,
            "resend_same_uuid_fresh_client": r2, "resend_status": second_status,
            "effects": rig.effects(order)}


def scenario_kill(rig: Rig, order: int, seed: int, after_effect: bool, same_client: bool) -> dict:
    rig.start_server(result_timeout=10)
    u = rig.uuid_from(seed)
    r1, h1 = rig.send(rig.ac, order, u)
    sent_at = time.time()
    kill_after = DURATION * (EFFECT_FRAC + 0.15 if after_effect else EFFECT_FRAC - 0.25)
    time.sleep(max(0.0, kill_after - (time.time() - sent_at)))
    effects_at_kill = rig.effects(order)
    rig.kill_server()
    time.sleep(1.5)
    rig.start_server(result_timeout=10)
    status_old = rig.result(h1) if h1 else "n/a"  # old goal id, restarted server
    client = rig.ac if same_client else rig.ac2
    if not same_client:
        client.wait_for_server(timeout_sec=10.0)
    r2, h2 = rig.send(client, order, u)  # resend the same goal id to the restarted server
    status_new = rig.result(h2) if h2 else "n/a"
    time.sleep(0.5)
    return {"first": r1, "effects_at_kill": effects_at_kill,
            "status_old_goal_after_restart": status_old,
            "resend_same_uuid": r2, "resend_status": status_new,
            "resend_via": "same_client" if same_client else "fresh_client",
            "effects": rig.effects(order), "client_exceptions": list(rig.client_errors)}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=20)
    ap.add_argument("--out", required=True)
    ap.add_argument("--only", default="")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    workdir = os.path.join(args.out, "work")  # kept with the results: ledger and server events
    os.makedirs(workdir, exist_ok=True)
    rig = Rig(workdir)

    rclpy_action = __import__("rclpy.action", fromlist=["ActionServer"])
    sig = inspect.signature(rclpy_action.ActionServer.__init__)
    record = {
        "layer": "1a (real rclpy action server; one RMW; one machine)",
        "rclpy_ActionServer_default_result_timeout_s": sig.parameters["result_timeout"].default,
        "rmw": os.environ.get("RMW_IMPLEMENTATION", "default (rmw_fastrtps_cpp)"),
        "ros_distro": os.environ.get("ROS_DISTRO"),
        "python": sys.version.split()[0],
        "platform": platform.platform(),
        "n": args.n,
        "duration_s": DURATION, "effect_frac": EFFECT_FRAC,
        "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    scenarios = {
        "dup_held": lambda o, s: scenario_dup_held(rig, o, s),
        "expiry_10s": lambda o, s: scenario_expiry(rig, o, s, 10),
        "kill_after_effect": lambda o, s: scenario_kill(rig, o, s, True, False),
        "kill_before_effect": lambda o, s: scenario_kill(rig, o, s, False, False),
        "kill_after_effect_same_client": lambda o, s: scenario_kill(rig, o, s, True, True),
    }
    wanted = [s for s in scenarios if not args.only or s in args.only.split(",")]
    path = os.path.join(args.out, "observations.jsonl")
    order = 0
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        for name in wanted:
            for i in range(args.n):
                order += 1
                t0 = time.time()
                rig.new_trial(order)
                try:
                    obs = scenarios[name](order, 0xA11CE000 + order)
                except Exception as exc:  # noqa: BLE001
                    obs = {"harness_error": f"{type(exc).__name__}: {exc}"}
                finally:
                    rig.end_trial()
                obs.update({"scenario": name, "trial": i, "order": order,
                            "seconds": round(time.time() - t0, 2),
                            "server_events": rig.timeline(order, t0)})
                fh.write(json.dumps(obs, sort_keys=True) + "\n")
                fh.flush()
                print(name, i, {k: v for k, v in obs.items() if k != "scenario"}, flush=True)
    record["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    with open(os.path.join(args.out, "run_record.json"), "w", encoding="utf-8", newline="\n") as fh:
        json.dump(record, fh, indent=2, sort_keys=True)
        fh.write("\n")
    rig._running = False
    rclpy.shutdown()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
