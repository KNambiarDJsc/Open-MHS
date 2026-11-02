"""The SDK as a real process: what its pipes and its exit do when things go wrong.

Each test starts an adapter as a child process and speaks the protocol to it directly.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import textwrap
import threading
import time
import unittest
from pathlib import Path

import support

PYTHON_DIR = str(support.REPO / "python")


class Child:
    """An adapter process, with bounded reads of its answers."""

    def __init__(self, body: str, tmp: Path) -> None:
        script = tmp / "adapter_under_test.py"
        script.write_text(textwrap.dedent(body), encoding="utf-8")
        self.proc = subprocess.Popen(
            [sys.executable, str(script)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            env={**os.environ, "PYTHONPATH": PYTHON_DIR},
        )
        self.next_id = 0

    def send(self, method: str, params: dict | None = None) -> int:
        self.next_id += 1
        line = json.dumps({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params or {}})
        self.proc.stdin.write(line.encode() + b"\n")
        self.proc.stdin.flush()
        return self.next_id

    def answer(self, timeout: float) -> dict | None:
        out: dict = {}
        reader = threading.Thread(target=lambda: out.setdefault("line", self.proc.stdout.readline()), daemon=True)
        reader.start()
        reader.join(timeout)
        return json.loads(out["line"]) if out.get("line") else None

    def call(self, method: str, params: dict | None = None, timeout: float = 60) -> dict | None:
        self.send(method, params)
        return self.answer(timeout)

    def start(self) -> None:
        answer = self.call("initialize", {"protocol_versions": ["0.1"]})
        assert answer and "result" in answer, answer

    def close(self) -> None:
        if self.proc.poll() is None:
            self.proc.kill()
        self.proc.wait()
        for stream in (self.proc.stdin, self.proc.stdout):
            try:
                stream.close()
            except OSError:
                pass


class ProcessTests(unittest.TestCase):
    def setUp(self) -> None:
        import tempfile

        self._dir = tempfile.TemporaryDirectory(prefix="openmhs-sdk-")
        self.addCleanup(self._dir.cleanup)
        self.tmp = Path(self._dir.name)

    def child(self, body: str) -> Child:
        c = Child(body, self.tmp)
        self.addCleanup(c.close)
        return c

    def test_an_adapter_stuck_in_a_blocking_call_still_exits_when_the_gateway_goes_away(self) -> None:
        # A gateway that is killed cannot kill its adapter. A blocking SDK call that never
        # returns must not leave the process behind, holding the device.
        c = self.child("""
            import asyncio, time
            from openmhs.adapter import Adapter, Applied, run

            class Stuck(Adapter):
                adapter_id = "stuck"
                async def execute(self, command, progress):
                    await asyncio.to_thread(time.sleep, 600)
                    return Applied({})

            run(Stuck())
        """)
        c.start()
        c.send("execute", {"command": {"command_id": "c", "capability": "x", "operation": "invoke",
                                       "idempotent": False, "deadline": "2099-01-01T00:00:00Z"}})
        time.sleep(0.5)
        c.proc.stdin.close()
        try:
            c.proc.wait(timeout=20)
        except subprocess.TimeoutExpired:
            self.fail("the adapter was still running 20 s after its gateway went away")

    def test_a_shutdown_that_hangs_does_not_keep_the_process_alive(self) -> None:
        c = self.child("""
            import asyncio
            from openmhs.adapter import Adapter, run

            class Hangs(Adapter):
                adapter_id = "hangs"
                async def shutdown(self):
                    await asyncio.Event().wait()

            run(Hangs())
        """)
        c.start()
        c.proc.stdin.close()
        try:
            c.proc.wait(timeout=20)
        except subprocess.TimeoutExpired:
            self.fail("a hanging shutdown() kept the adapter alive")

    def test_child_processes_and_native_code_do_not_see_the_protocol_pipe(self) -> None:
        c = self.child("""
            import os, stat
            from openmhs.adapter import Adapter, reading, run

            class Probe(Adapter):
                adapter_id = "probe"
                async def read(self, capability):
                    return reading(stat.S_ISFIFO(os.fstat(0).st_mode), source="simulator")

            run(Probe())
        """)
        c.start()
        answer = c.call("read", {"capability": "x"})
        self.assertIs(answer["result"]["value"], False, "file descriptor 0 is still the protocol's pipe")

    def test_a_native_extension_can_be_imported_after_the_sdk_started(self) -> None:
        # Hung forever on Windows while the SDK read the inherited stdin
        # (research/lab-automation/02_opentrons, finding 1).
        try:
            import numpy  # noqa: F401
        except ImportError:
            self.skipTest("numpy is not installed")
        c = self.child("""
            import asyncio
            from openmhs.adapter import Adapter, reading, run

            class Late(Adapter):
                adapter_id = "late"
                async def read(self, capability):
                    def load():
                        import numpy
                        return numpy.__version__
                    return reading(await asyncio.to_thread(load), source="simulator")

            run(Late())
        """)
        c.start()
        answer = c.call("read", {"capability": "x"}, timeout=45)
        self.assertIsNotNone(answer, "no answer: the import hung")
        self.assertIn("result", answer)

    def test_requests_keep_flowing_through_the_duplicated_pipe(self) -> None:
        c = self.child("""
            from openmhs.adapter import Adapter, reading, run

            class Echo(Adapter):
                adapter_id = "echo"
                async def read(self, capability):
                    return reading(capability, source="simulator")

            run(Echo())
        """)
        c.start()
        for name in ("a", "b", "c"):
            self.assertEqual(c.call("read", {"capability": name})["result"]["value"], name)
        self.assertEqual(c.call("shutdown")["result"], None)
        self.assertEqual(c.proc.wait(timeout=20), 0)


if __name__ == "__main__":
    unittest.main()
