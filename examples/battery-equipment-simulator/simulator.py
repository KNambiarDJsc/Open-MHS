"""A SIMULATED battery test instrument that speaks Modbus TCP. Standard library only.

It serves the EXAMPLE-ONLY register map in ``register_map.py`` on localhost, from a
deterministic fixture. It is read-only at the device level: every write function code is
refused with exception 01 (illegal function) and recorded, so even a gateway that wrongly
forwarded a write could not change anything here.

    python simulator.py --fixture session            # serve a fixture on 127.0.0.1:15020

Not a model of any real instrument and not a battery model. Values are fixed by the fixture.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import struct
import threading
import time
from pathlib import Path
from typing import Any, Callable

import register_map as rm

HERE = Path(__file__).resolve().parent
FIXTURES = HERE / "fixtures"

ILLEGAL_FUNCTION = 0x01
ILLEGAL_ADDRESS = 0x02
ILLEGAL_VALUE = 0x03
WRITE_FUNCTIONS = {0x05, 0x06, 0x0F, 0x10, 0x16, 0x17}


def load_fixture(name_or_path: str) -> dict[str, Any]:
    path = Path(name_or_path)
    if not path.suffix:
        path = FIXTURES / f"{name_or_path}.json"
    fixture = json.loads(path.read_text(encoding="utf-8"))
    fixture.setdefault("name", path.stem)
    return fixture


class BatterySimulator:
    """Serve a fixture. ``start()`` runs the server on its own thread."""

    def __init__(self, fixture: dict[str, Any], host: str = "127.0.0.1", port: int = 0,
                 clock: Callable[[], float] = time.time) -> None:
        self.host = host
        self.port = port
        self._clock = clock
        self._regs = rm.by_name()
        self._tables: dict[str, dict[int, int]] = {}
        self._age_s = 0.0
        self.link_up = True
        self.reads = 0
        self.write_attempts: list[dict[str, int]] = []
        self._writers: set[asyncio.StreamWriter] = set()
        self._loop: asyncio.AbstractEventLoop | None = None
        self._server: asyncio.base_events.Server | None = None
        self._ready = threading.Event()
        self.load(fixture)

    # ------------------------------------------------------------------ fixtures

    def load(self, fixture: dict[str, Any]) -> None:
        """Switch scenario. Unlisted registers read as zero (or the first enum value)."""
        tables: dict[str, dict[int, int]] = {t: {} for t in ("coil", "discrete_input", "input", "holding")}
        for reg in self._regs.values():
            for i in range(reg.width):
                tables[reg.table][reg.address + i] = 0
        values = {"instrument.simulated": True, **fixture.get("values", {})}
        for name, value in values.items():
            reg = self._regs[name]
            for i, word in enumerate(rm.encode(reg, value)):
                tables[reg.table][reg.address + i] = word
        for name, words in fixture.get("raw", {}).items():
            reg = self._regs[name]
            if len(words) != reg.width:
                raise ValueError(f"{name}: expected {reg.width} raw word(s)")
            for i, word in enumerate(words):
                tables[reg.table][reg.address + i] = word & 0xFFFF
        self._tables = tables
        self._age_s = float(fixture.get("sample_age_s", 0))
        self.fixture_name = fixture.get("name", "unnamed")

    def _word(self, table: str, address: int) -> int:
        reg = self._regs["telemetry.sample_time"]
        if table == "input" and address in (reg.address, reg.address + 1):
            now = int(self._clock() - self._age_s)  # the instrument last updated `age` ago
            return rm.encode(reg, now)[address - reg.address]
        return self._tables[table][address]

    # ------------------------------------------------------------------ protocol

    def answer(self, pdu: bytes) -> bytes:
        fc = pdu[0] if pdu else 0
        if fc in WRITE_FUNCTIONS:
            address = struct.unpack(">H", pdu[1:3])[0] if len(pdu) >= 3 else -1
            self.write_attempts.append({"function": fc, "address": address})
            return bytes([fc | 0x80, ILLEGAL_FUNCTION])  # read-only instrument
        if fc not in (0x01, 0x02, 0x03, 0x04) or len(pdu) != 5:
            return bytes([fc | 0x80, ILLEGAL_FUNCTION])
        start, count = struct.unpack(">HH", pdu[1:5])
        table = {0x01: "coil", 0x02: "discrete_input", 0x03: "holding", 0x04: "input"}[fc]
        if not 1 <= count <= (2000 if fc in (1, 2) else 125):
            return bytes([fc | 0x80, ILLEGAL_VALUE])
        if any(a not in self._tables[table] for a in range(start, start + count)):
            return bytes([fc | 0x80, ILLEGAL_ADDRESS])
        self.reads += 1
        words = [self._word(table, a) for a in range(start, start + count)]
        if fc in (1, 2):
            data = bytearray((count + 7) // 8)
            for i, w in enumerate(words):
                if w:
                    data[i // 8] |= 1 << (i % 8)
            return bytes([fc, len(data)]) + bytes(data)
        return bytes([fc, count * 2]) + b"".join(struct.pack(">H", w) for w in words)

    async def _handle(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        if not self.link_up:
            writer.close()
            return
        self._writers.add(writer)
        try:
            while self.link_up:
                header = await reader.readexactly(7)
                tid, pid, length, unit = struct.unpack(">HHHB", header)
                if pid != 0 or not 2 <= length <= 254:
                    break
                pdu = await reader.readexactly(length - 1)
                if not self.link_up:
                    break
                reply = self.answer(pdu)
                writer.write(struct.pack(">HHHB", tid, 0, len(reply) + 1, unit) + reply)
                await writer.drain()
        except (asyncio.IncompleteReadError, ConnectionError):
            pass
        finally:
            self._writers.discard(writer)
            writer.close()

    # ------------------------------------------------------------------ lifecycle

    def start(self) -> int:
        threading.Thread(target=self._run, daemon=True).start()
        if not self._ready.wait(10):
            raise RuntimeError("simulator did not start")
        return self.port

    def _run(self) -> None:
        self._loop = asyncio.new_event_loop()
        asyncio.set_event_loop(self._loop)

        async def main() -> None:
            self._server = await asyncio.start_server(self._handle, self.host, self.port)
            self.port = self._server.sockets[0].getsockname()[1]
            self._ready.set()
            async with self._server:
                await self._server.serve_forever()

        try:
            self._loop.run_until_complete(main())
        except asyncio.CancelledError:
            pass
        finally:
            self._loop.close()

    def set_link(self, up: bool) -> None:
        """Take the instrument off the network, or bring it back. Open connections drop."""
        self.link_up = up
        if not up and self._loop:
            def drop() -> None:
                for w in list(self._writers):
                    w.close()
            self._loop.call_soon_threadsafe(drop)

    def stop(self) -> None:
        if self._loop and self._server:
            def shut() -> None:
                assert self._server is not None
                self._server.close()
                for w in list(self._writers):
                    w.close()
                for task in asyncio.all_tasks():
                    task.cancel()
            self._loop.call_soon_threadsafe(shut)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--fixture", default="normal", help="name in fixtures/ or a path")
    ap.add_argument("--port", type=int, default=15020)
    args = ap.parse_args()
    sim = BatterySimulator(load_fixture(args.fixture), port=args.port)
    port = sim.start()
    print(f"SIMULATED battery test instrument, fixture '{sim.fixture_name}', Modbus TCP on 127.0.0.1:{port}")
    print("read-only; EXAMPLE-ONLY register map; Ctrl+C to stop")
    try:
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        sim.stop()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
