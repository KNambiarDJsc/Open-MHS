"""Model of ROS 2 action semantics, in virtual time. NOT ROS 2.

Derived from reading the ROS 2 action design and rcl/rclcpp sources (see PREREGISTRATION.md):
a client-generated goal UUID, an in-memory goal table, duplicate-UUID rejection while the goal
exists, result expiry (default 10 s) after the goal finishes, STATUS_UNKNOWN for any goal the
server does not currently hold, and no persistence across a server restart.
"""

from __future__ import annotations

import random
from dataclasses import dataclass, field

import channel

RTT = 0.05
RESULT_EXPIRY = 10.0
T_SEND = 1.0
T_RES = 8.0
OP_DEADLINE = 60.0
MEASURE_SETTLE = 6.0
MEASURE_SPACING = 0.5

SUCCEEDED = "SUCCEEDED"
ABORTED = "ABORTED"
EXECUTING = "EXECUTING"
UNKNOWN = "UNKNOWN"

_ORDER = {"eff": 0, "done": 1, "expire": 2, "kill": 3, "up": 4}


class ClientCrash(Exception):
    """Raised inside a client when the injected client crash (F6) happens."""


@dataclass
class Params:
    duration: float
    effect_frac: float
    kill_frac: float
    restart_delay: float
    stall: float
    crash_offset: float
    crash_restart: float
    meas_seed: int


def draw_params(seed: int) -> Params:
    """All per-trial random quantities, drawn once and independent of the arm."""
    rng = random.Random(seed)
    # Draw order is fixed so a trial's parameters never depend on which fault or arm is run.
    duration = rng.uniform(2.0, 5.0)
    effect_frac = rng.uniform(0.2, 0.8)
    kill_frac = rng.uniform(0.0, 1.0)
    restart_delay = rng.uniform(1.0, 3.0)
    stall = rng.uniform(12.0, 30.0)
    crash_offset = rng.uniform(0.1, duration + 1.0)
    crash_restart = rng.uniform(1.0, 3.0)
    meas_seed = rng.getrandbits(48)
    return Params(duration, effect_frac, kill_frac, restart_delay, stall, crash_offset,
                  crash_restart, meas_seed)


class Plant:
    """Ground truth. The ledger records every effect event; no client may read it."""

    def __init__(self) -> None:
        self.ledger: list[tuple[float, str]] = []

    def apply(self, t: float, op_id: str) -> None:
        self.ledger.append((t, op_id))

    def count(self, op_id: str) -> int:
        return sum(1 for _, o in self.ledger if o == op_id)


@dataclass
class Goal:
    uuid: str
    op_id: str
    accepted_at: float
    t_eff: float
    t_done: float
    lie: bool = False
    status: str = EXECUTING
    effect_done: bool = False
    finished_at: float = 0.0


class Server:
    def __init__(self, plant: Plant, expiry: float = RESULT_EXPIRY) -> None:
        self.plant = plant
        self.expiry = expiry
        self.goals: dict[str, Goal] = {}
        self.down = False
        self.kill_at: float | None = None
        self.restart_delay = 0.0
        self.up_at: float | None = None
        self.kills = 0
        self.lie_next = False
        self.lies_executed = 0
        self.accepted_total = 0

    def schedule_kill(self, t_kill: float, restart_delay: float) -> None:
        self.kill_at = t_kill
        self.restart_delay = restart_delay

    def _next_event(self, limit: float):
        cands = []
        for g in self.goals.values():
            if g.status == EXECUTING:
                if not g.effect_done:
                    cands.append((g.t_eff, "eff", g))
                cands.append((g.t_done, "done", g))
            else:
                cands.append((g.finished_at + self.expiry, "expire", g))
        if self.kill_at is not None and not self.down:
            cands.append((self.kill_at, "kill", None))
        if self.down and self.up_at is not None:
            cands.append((self.up_at, "up", None))
        cands = [c for c in cands if c[0] <= limit]
        if not cands:
            return None
        return min(cands, key=lambda c: (c[0], _ORDER[c[1]]))

    def advance(self, limit: float) -> None:
        while True:
            ev = self._next_event(limit)
            if ev is None:
                return
            t, kind, g = ev
            if kind == "eff":
                g.effect_done = True
                if g.lie:
                    self.lies_executed += 1
                else:
                    self.plant.apply(t, g.op_id)
            elif kind == "done":
                g.status = SUCCEEDED
                g.finished_at = t
            elif kind == "expire":
                self.goals.pop(g.uuid, None)
            elif kind == "kill":
                self.goals.clear()  # in-memory table and any goal in flight are lost
                self.down = True
                self.kills += 1
                self.up_at = t + self.restart_delay
                self.kill_at = None
            elif kind == "up":
                self.down = False
                self.up_at = None

    def accept(self, t: float, uuid: str, op_id: str, params: Params) -> str:
        if uuid in self.goals:
            return "REJECTED"  # duplicate goal id while the goal is still held
        lie = self.lie_next
        self.lie_next = False
        self.goals[uuid] = Goal(
            uuid=uuid, op_id=op_id, accepted_at=t,
            t_eff=t + params.effect_frac * params.duration,
            t_done=t + params.duration, lie=lie,
        )
        self.accepted_total += 1
        return "ACCEPTED"


@dataclass
class Env:
    """One trial's runtime: clock, server, plant, nemesis state and client-visible history."""

    fault: str
    params: Params
    plant: Plant = field(default_factory=Plant)
    now: float = 0.0
    history: list[dict] = field(default_factory=list)

    def __post_init__(self) -> None:
        self.server = Server(self.plant)
        self._uuid_counter = 0
        self.n_sends = 0
        self.first_accept_done = False
        self.f1_pending = self.fault == "F1"
        self.f2_pending = self.fault == "F2"
        self.f4_pending = self.fault == "F4"
        self.f5_pending = self.fault == "F5"
        self.f3_pending = self.fault == "F3"
        self.f1_fired = self.f2_fired = self.f4_fired = False
        self.crash_at: float | None = None
        self.crashed = False
        self.meas_rng = random.Random(self.params.meas_seed)
        if self.f5_pending:
            self.server.lie_next = True

    # -- helpers -------------------------------------------------------------------------
    def new_uuid(self) -> str:
        self._uuid_counter += 1
        return f"g{self._uuid_counter}"

    def log(self, event: str, op_id: str, **kw) -> None:
        self.history.append({"t": round(self.now, 6), "event": event, "op": op_id, **kw})

    def _after_rpc(self) -> None:
        if self.crash_at is not None and not self.crashed and self.now >= self.crash_at:
            self.crashed = True
            self.now = max(self.now, self.crash_at + self.params.crash_restart)
            self.server.advance(self.now)
            raise ClientCrash()

    # -- RPCs ----------------------------------------------------------------------------
    def send_goal(self, uuid: str, op_id: str) -> str | None:
        """Returns 'ACCEPTED', 'REJECTED' or None when no response arrives."""
        t0 = self.now
        if self.n_sends == 0 and self.fault == "F6":
            self.crash_at = t0 + self.params.crash_offset
        self.n_sends += 1
        self.server.advance(t0)
        if self.server.down:
            result: str | None = None
            self.now = t0 + T_SEND
        else:
            result = self.server.accept(t0, uuid, op_id, self.params)
            if result == "ACCEPTED":
                if self.f3_pending:
                    self.server.schedule_kill(t0 + self.params.kill_frac * self.params.duration,
                                              self.params.restart_delay)
                    self.f3_pending = False
                if self.f1_pending:
                    self.f1_pending = False
                    self.f1_fired = True
                    result = None
                    self.now = t0 + T_SEND
                else:
                    self.now = t0 + RTT
                    if self.f4_pending and not self.first_accept_done:
                        self.f4_pending = False
                        self.f4_fired = True
                        self.now += self.params.stall
                self.first_accept_done = True
            else:
                self.now = t0 + RTT
        self.server.advance(self.now)
        self.log("send", op_id, uuid=uuid, response=result)
        self._after_rpc()
        return result

    def get_result(self, uuid: str, op_id: str) -> str | None:
        """Returns SUCCEEDED / ABORTED / UNKNOWN, or None on timeout (no response)."""
        t0 = self.now
        self.server.advance(t0)
        status: str | None
        if self.server.down:
            status = None
            self.now = t0 + T_RES
        else:
            g = self.server.goals.get(uuid)
            if g is None:
                status = UNKNOWN
                self.now = t0 + RTT
            else:
                if g.status == EXECUTING:
                    t_resp = g.t_done + RTT
                    if t_resp - t0 > T_RES:
                        status = None
                        self.now = t0 + T_RES
                    else:
                        self.server.advance(t_resp)
                        g2 = self.server.goals.get(uuid)
                        if g2 is None or self.server.down:
                            status = None  # server died while the client was waiting
                            self.now = t0 + T_RES
                        else:
                            status = g2.status
                            self.now = t_resp
                else:
                    status = g.status
                    self.now = t0 + RTT
            if status is not None and self.f2_pending:
                self.f2_pending = False
                self.f2_fired = True
                status = None
                self.now = t0 + T_RES
        self.server.advance(self.now)
        self.log("get_result", op_id, uuid=uuid, response=status)
        self._after_rpc()
        return status

    def measure(self, op_id: str) -> list:
        """Independent measurement: wait for the system to settle, then take L noisy readings."""
        self.now += MEASURE_SETTLE
        readings: list = []
        for _ in range(channel.N_READINGS):
            self.now += MEASURE_SPACING
            self.server.advance(self.now)
            u_drop = self.meas_rng.random()
            u_val = self.meas_rng.random()
            if u_drop < channel.DROPOUT:
                readings.append(None)
                continue
            applied = self.plant.count(op_id) >= 1
            p1 = (1.0 - channel.EPS_FN) if applied else channel.EPS_FP
            readings.append(1 if u_val < p1 else 0)
        self.log("info", op_id, readings=readings)
        self._after_rpc()
        return readings

    # -- reporting -----------------------------------------------------------------------
    def fault_triggered(self) -> bool:
        return {
            "F0": False,
            "F1": self.f1_fired,
            "F2": self.f2_fired,
            "F3": self.server.kills >= 1,
            "F4": self.f4_fired,
            "F5": self.server.lies_executed >= 1,
            "F6": self.crashed,
        }[self.fault]
