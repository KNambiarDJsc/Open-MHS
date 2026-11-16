"""A small decoder for the ROS 2 messages the importers need, standard library only.

ROS 2 serializes messages with CDR (little endian, "XCDR1"): a 4-byte encapsulation header, then fields, each aligned to
its own size counted from the first byte after the header. Only these standard types are read, and only the fields the
importers use; anything else is refused rather than guessed:

- ``std_msgs/msg/String``
- ``action_msgs/msg/GoalStatusArray`` (goal id, accept time, status of every goal the server tracks)
- ``geometry_msgs/msg/PoseStamped`` (header stamp, frame id, position, orientation)

Test vectors in ``python/tests/test_cdr.py`` were produced by the reference ROS 2 MCAP encoder.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass

GOAL_STATUS = {0: "UNKNOWN", 1: "ACCEPTED", 2: "EXECUTING", 3: "CANCELING", 4: "SUCCEEDED", 5: "CANCELED", 6: "ABORTED"}


class CdrError(ValueError):
    """The bytes are not the message they were said to be."""


class _Reader:
    def __init__(self, data: bytes):
        if len(data) < 4:
            raise CdrError("shorter than the CDR header")
        if data[1] != 1:
            raise CdrError("only little-endian CDR is read (header byte 1 must be 1)")
        self.data = data
        self.pos = 4
        self.base = 4

    def _align(self, size: int) -> None:
        self.pos += (-(self.pos - self.base)) % size

    def _take(self, fmt: str, size: int):
        self._align(size)
        if self.pos + size > len(self.data):
            raise CdrError("message ends early")
        (value,) = struct.unpack_from("<" + fmt, self.data, self.pos)
        self.pos += size
        return value

    def u32(self) -> int:
        return self._take("I", 4)

    def i32(self) -> int:
        return self._take("i", 4)

    def i8(self) -> int:
        return self._take("b", 1)

    def f64(self) -> float:
        return self._take("d", 8)

    def raw(self, n: int) -> bytes:
        if self.pos + n > len(self.data):
            raise CdrError("message ends early")
        out = self.data[self.pos : self.pos + n]
        self.pos += n
        return out

    def string(self) -> str:
        n = self.u32()
        if n == 0:
            raise CdrError("a CDR string is at least its terminating NUL")
        body = self.raw(n)
        if body[-1] != 0:
            raise CdrError("string is not NUL terminated")
        return body[:-1].decode("utf-8", errors="replace")


def decode_string(data: bytes) -> str:
    """``std_msgs/msg/String`` → its text."""
    return _Reader(data).string()


@dataclass(frozen=True)
class GoalStatus:
    goal_id: str  # canonical UUID text
    accepted_sec: int
    accepted_nanosec: int
    status: str  # one of GOAL_STATUS' names


def _uuid_text(b: bytes) -> str:
    h = b.hex()
    return f"{h[:8]}-{h[8:12]}-{h[12:16]}-{h[16:20]}-{h[20:]}"


def decode_goal_status_array(data: bytes) -> list[GoalStatus]:
    """``action_msgs/msg/GoalStatusArray`` → its goals."""
    r = _Reader(data)
    count = r.u32()
    if count > 100_000:
        raise CdrError(f"implausible goal count {count}")
    out = []
    for _ in range(count):
        goal_id = _uuid_text(r.raw(16))
        sec, nsec = r.i32(), r.u32()
        code = r.i8()
        out.append(GoalStatus(goal_id, sec, nsec, GOAL_STATUS.get(code, f"CODE_{code}")))
    return out


@dataclass(frozen=True)
class PoseStamped:
    sec: int
    nanosec: int
    frame_id: str
    position: tuple[float, float, float]
    orientation: tuple[float, float, float, float]


def decode_pose_stamped(data: bytes) -> PoseStamped:
    """``geometry_msgs/msg/PoseStamped``."""
    r = _Reader(data)
    sec, nsec = r.i32(), r.u32()
    frame = r.string()
    position = (r.f64(), r.f64(), r.f64())
    orientation = (r.f64(), r.f64(), r.f64(), r.f64())
    return PoseStamped(sec, nsec, frame, position, orientation)
