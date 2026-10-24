"""The OctoPrint adapter against OctoPrint's real answers.

Offline part (always runs): the adapter against a fake OctoPrint that replays answers
recorded from a real one (``fixtures/``), with faults injected by a recording proxy.

Real part (runs when ``OPENMHS_OCTOPRINT`` names an OctoPrint 1.x executable, see
requirements.txt): the same checks against a running OctoPrint with its Virtual Printer, plus
a disconnect and a reconnect of that printer.

Gateway part (also needs ``OPENMHS_BIN``, the ``openmhs`` binary): the adapter behind a real
gateway, with grants, denials, the audit log and the lifecycle of a refused write.

    python -m unittest discover -s adapters/python/octoprint -v

Everything stays on loopback. No hardware is involved: the "printer" is a plugin that
OctoPrint ships. The tests never send G-code from the adapter; the only non-GET requests
are the harness's own, to disconnect and reconnect the virtual printer.
"""

import asyncio
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(HERE))

import octoprint_adapter as oa  # noqa: E402
import octoprint_harness as h  # noqa: E402
from openmhs.adapter import AdapterError, Permissions  # noqa: E402

MANIFEST = json.loads((HERE / "manifest.json").read_text(encoding="utf-8"))
DESCRIPTOR = MANIFEST["config"]["descriptor"]
FIXTURES = {p.stem: json.loads(p.read_text(encoding="utf-8")) for p in (HERE / "fixtures").glob("*.json")}
CAPABILITIES = [c["name"] for c in DESCRIPTOR["capabilities"]]
KEY_ENV = "OCTOPRINT_API_KEY"


def run(coro):
    return asyncio.run(coro)


def adapter_for(url: str, key: str, **config):
    """An initialised adapter whose manifest allows exactly ``url``."""
    adapter = oa.OctoPrintAdapter()
    cfg = {"url": url, "timeout_ms": config.pop("timeout_ms", 1500), "descriptor": config.pop("descriptor", DESCRIPTOR)}
    cfg.update(config)
    with mock.patch.dict(os.environ, {KEY_ENV: key}):
        run(adapter.initialize(cfg, Permissions(network=(url,))))
    return adapter


def kind_of(coro) -> str:
    try:
        run(coro)
    except AdapterError as e:
        return e.kind
    return "no error"


# --- A fake OctoPrint that replays recorded answers -------------------------------------


class FakeOctoPrint:
    """Serves the recorded ``/api/connection`` and ``/api/printer`` answers. ``closed``
    switches to what OctoPrint said while its printer connection was closed."""

    def __init__(self, key: str):
        self.key = key
        self.closed = False
        self.requests: list = []
        fake = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                path = self.path.partition("?")[0]
                fake.requests.append(("GET", path))
                if self.headers.get("X-Api-Key") != fake.key:
                    return self.reply(403, {"error": "no permission"})
                name = {"/api/connection": "connection", "/api/printer": "printer"}.get(path)
                if name is None:
                    return self.reply(404, {"error": "not found"})
                if fake.closed:
                    fx = FIXTURES["connection-closed" if name == "connection" else "printer-offline-409"]
                else:
                    fx = FIXTURES[f"{name}-operational"]
                self.reply(fx["status"], fx["body"])

            def do_POST(self):
                fake.requests.append(("POST", self.path))
                self.reply(500, {"error": "the fake accepts no writes"})

            do_PUT = do_DELETE = do_PATCH = do_POST

            def reply(self, status, body):
                data = json.dumps(body).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


# --- Mapping and configuration (no network) ----------------------------------------------


class MappingTests(unittest.TestCase):
    def test_recorded_answers_map_to_capabilities(self):
        conn = FIXTURES["connection-operational"]["body"]
        self.assertEqual(oa.connection_state(conn), ("operational", "simulator"))
        printer = FIXTURES["printer-operational"]["body"]
        value, measured = oa.temperature(printer, "hotend.temperature")
        self.assertIsInstance(value, float)
        self.assertIsNotNone(measured)
        self.assertEqual(oa.temperature(printer, "bed.target")[0], 0.0)

    def test_closed_connection_is_offline_and_still_a_simulator(self):
        self.assertEqual(oa.connection_state(FIXTURES["connection-closed"]["body"]), ("offline", "simulator"))

    def test_states(self):
        def state(text, port="COM3"):
            return oa.connection_state({"current": {"state": text, "port": port}})

        self.assertEqual(state("Printing"), ("printing", "device"))
        self.assertEqual(state("Pausing")[0], "paused")
        self.assertEqual(state("Cancelling")[0], "printing")
        self.assertEqual(state("Opening serial connection")[0], "connecting")
        self.assertEqual(state("Error: Printer on fire")[0], "error")
        self.assertEqual(state("Closed with error: x")[0], "error")
        with self.assertRaises(AdapterError) as e:
            state("Levitating")
        self.assertEqual(e.exception.kind, "malformed_response")

    def test_malformed_bodies(self):
        bad = [
            {},
            {"temperature": {"tool0": {"actual": "hot"}}},
            {"temperature": {"tool0": {"actual": True}}},
            {"temperature": {"tool0": {"actual": float("nan")}}},
        ]
        for body in bad:
            with self.subTest(body=body), self.assertRaises(AdapterError) as e:
                oa.temperature(body, "hotend.temperature")
            self.assertEqual(e.exception.kind, "malformed_response")

    def test_a_temperature_that_is_not_reported_yet_is_a_device_failure(self):
        """Seen from a real OctoPrint right after the printer connects: ``actual`` is null."""
        body = FIXTURES.get("printer-just-connected", {}).get("body") or {
            "temperature": {"tool0": {"actual": None, "target": None}, "bed": {"actual": None, "target": None}}
        }
        for name in oa.TEMPERATURES:
            with self.assertRaises(AdapterError) as e:
                oa.temperature(body, name)
            self.assertEqual(e.exception.kind, "device_failure", name)

    def test_null_temperature_is_not_yet_reported_not_malformed(self):
        # OctoPrint answers null until the printer's first temperature poll after connecting.
        body = {"temperature": {"tool0": {"actual": None, "target": None}, "bed": {"actual": None, "target": None}}}
        for cap in oa.TEMPERATURES:
            with self.assertRaises(AdapterError) as e:
                oa.temperature(body, cap)
            self.assertEqual(e.exception.kind, "device_failure", cap)

    def test_every_capability_is_read_only_and_known(self):
        for cap in DESCRIPTOR["capabilities"]:
            self.assertEqual(cap["operations"], ["read"], cap["name"])
            self.assertEqual(cap["side_effect"], "none", cap["name"])
        self.assertEqual(set(CAPABILITIES), set(oa.TEMPERATURES) | {"printer.state"})


class ConfigTests(unittest.TestCase):
    def init(self, config, perms=None, env=None):
        adapter = oa.OctoPrintAdapter()
        env = {KEY_ENV: "k"} if env is None else env
        with mock.patch.dict(os.environ, env):
            try:
                run(adapter.initialize(config, perms or Permissions(network=(config.get("url", ""),))))
            except AdapterError as e:
                return e
        return None

    def base(self, **extra):
        return {"url": "http://127.0.0.1:5000", "descriptor": DESCRIPTOR, **extra}

    def test_accepts_a_loopback_url(self):
        self.assertIsNone(self.init(self.base()))

    def test_refuses_a_descriptor_that_writes(self):
        d = json.loads(json.dumps(DESCRIPTOR))
        d["capabilities"][1]["operations"] = ["read", "write"]
        err = self.init(self.base(descriptor=d))
        self.assertEqual(err.kind, "internal")
        self.assertIn("read-only", err.message)

    def test_refuses_a_capability_it_cannot_read(self):
        d = json.loads(json.dumps(DESCRIPTOR))
        d["capabilities"].append({**d["capabilities"][1], "name": "job.cancel", "operations": ["read"]})
        self.assertIn("cannot read", self.init(self.base(descriptor=d)).message)

    def test_refuses_plain_http_off_loopback(self):
        url = "http://192.0.2.7:5000"
        self.assertIn("plain HTTP", self.init(self.base(url=url), Permissions(network=(url,))).message)
        self.assertIsNone(self.init(self.base(url=url, allow_plaintext=True), Permissions(network=(url,))))

    def test_refuses_an_endpoint_the_manifest_does_not_declare(self):
        err = self.init(self.base(), Permissions(network=("http://127.0.0.1:9999",)))
        self.assertIn("not among the manifest's endpoints.network", err.message)

    def test_refuses_to_start_without_its_credential(self):
        err = self.init(self.base(), env={KEY_ENV: ""})
        self.assertIn("credential", err.message)

    def test_refuses_unknown_keys(self):
        self.assertIn("unknown config keys", self.init(self.base(apikey="x")).message)


class ReadOnlyByConstruction(unittest.TestCase):
    def test_the_client_requests_only_the_two_read_paths(self):
        client = oa.OctoPrintClient("http://127.0.0.1:1", "k", 0.2)
        for path in ("/api/printer/command", "/api/job", "/api/files/local", "/api/connection/../job", "/"):
            with self.subTest(path=path), self.assertRaises(AdapterError) as e:
                client.get(path)
            self.assertEqual(e.exception.kind, "internal")
        self.assertEqual(client.sent, [])
        self.assertEqual(oa.GET_PATHS, {"/api/connection", "/api/printer"})

    def test_the_source_has_no_way_to_write(self):
        source = (HERE / "octoprint_adapter.py").read_text(encoding="utf-8")
        code = "\n".join(line for line in source.splitlines() if not line.lstrip().startswith(("#", '"""')))
        for forbidden in ("POST", "PUT", "DELETE", "PATCH", "printer/command", "/api/job", "/api/files", 'method="'):
            if forbidden == 'method="':
                self.assertEqual(re.findall(r'method="(\w+)"', code), ["GET"])
            else:
                self.assertNotIn(forbidden, code, forbidden)

    def test_execute_is_unsupported(self):
        a = adapter_for("http://127.0.0.1:5000", "k")
        self.assertEqual(kind_of(a.execute(None, None)), "unsupported")


# --- Faults: how each failure of the HTTP link is classified ------------------------------

#: mode on the proxy -> the error class the adapter must report.
FAULTS = [
    (("drop",), "disconnected"),
    (("delay", 3), "timeout"),
    (("status", 403), "device_failure"),
    (("status", 401), "device_failure"),
    (("status", 404), "device_failure"),
    (("status", 500), "device_failure"),
    (("status", 503), "device_failure"),
    (("redirect", "http://127.0.0.1:1/steal"), "device_failure"),
    (("body", b"<html>not json</html>"), "malformed_response"),
    (("body", b'{"current": {"state": "Operat'), "malformed_response"),
    (("body", b"[1, 2, 3]"), "malformed_response"),
    (("body", b"{}"), "malformed_response"),
    (("body", b"x" * (oa.MAX_BODY + 10)), "malformed_response"),
]


class FaultClassification(unittest.TestCase):
    """Run against whichever upstream the subclass provides."""

    upstream_url = None
    key = None

    @classmethod
    def setUpClass(cls):
        if cls is FaultClassification:
            raise unittest.SkipTest("abstract")
        cls.proxy = h.RecordingProxy(cls.upstream_url).__enter__()

    @classmethod
    def tearDownClass(cls):
        cls.proxy.__exit__()

    def setUp(self):
        self.proxy.mode = ("pass",)
        self.proxy.reset()

    def adapter(self):
        return adapter_for(self.proxy.url, self.key, timeout_ms=700)

    def test_each_fault_has_one_class_on_every_path(self):
        for mode, expected in FAULTS:
            for capability in ("printer.state", "hotend.temperature"):
                with self.subTest(mode=mode[0], detail=str(mode[1:])[:30], capability=capability):
                    a = self.adapter()
                    self.proxy.mode = mode
                    self.assertEqual(kind_of(a.read(capability)), expected)

    def test_health_under_faults(self):
        for mode, expected in FAULTS:
            with self.subTest(mode=mode[0], detail=str(mode[1:])[:30]):
                a = self.adapter()
                self.proxy.mode = mode
                status, why = run(a.health())
                self.assertEqual(status, "disconnected" if expected in ("disconnected", "timeout") else "degraded")
                self.assertTrue(why)
                self.assertNotIn(self.key, why)

    def test_recovery_when_the_fault_clears(self):
        a = self.adapter()
        self.proxy.mode = ("drop",)
        self.assertEqual(kind_of(a.read("hotend.temperature")), "disconnected")
        self.proxy.mode = ("pass",)
        self.assertEqual(run(a.health()), "healthy")
        self.assertEqual(run(a.read("hotend.temperature"))["quality"], "GOOD")

    def test_no_fault_ever_causes_a_write(self):
        """The adapter has no outcome that is uncertain: whatever the link does, the only
        requests it can have sent are GETs to the two read paths."""
        for mode, _ in FAULTS:
            a = self.adapter()
            self.proxy.mode = mode
            kind_of(a.read("hotend.temperature"))
            run(a.health())
        self.assertGreater(self.proxy.count(), 0)
        self.assertEqual(self.proxy.methods(), {"GET"})
        with self.proxy.lock:
            self.assertLessEqual({p for _, p, _ in self.proxy.requests}, set(oa.GET_PATHS))

    def test_fixture_link_down_is_reported_and_cleared(self):
        a = self.adapter()
        run(a.fixture_inject("link_down"))
        self.assertEqual(kind_of(a.read("printer.state")), "disconnected")
        self.assertEqual(run(a.health())[0], "disconnected")
        before = self.proxy.count()
        kind_of(a.read("printer.state"))
        self.assertEqual(self.proxy.count(), before, "a downed link must not reach the network")
        run(a.fixture_clear())
        self.assertEqual(run(a.read("printer.state"))["quality"], "GOOD")
        self.assertEqual(kind_of(a.fixture_inject("lost_ack")), "unsupported")


class FaultsAgainstReplay(FaultClassification):
    @classmethod
    def setUpClass(cls):
        cls.key = "replay-key-0123456789"
        cls.fake = FakeOctoPrint(cls.key)
        cls.upstream_url = cls.fake.url
        super().setUpClass()

    @classmethod
    def tearDownClass(cls):
        super().tearDownClass()
        cls.fake.close()

    def test_a_wrong_key_is_a_device_failure_not_a_disconnect(self):
        a = adapter_for(self.proxy.url, "not-the-key")
        self.assertEqual(kind_of(a.read("printer.state")), "device_failure")
        self.assertEqual(run(a.health())[0], "degraded")

    def test_replayed_closed_printer(self):
        a = self.adapter()
        self.fake.closed = True
        try:
            self.assertEqual(run(a.health())[0], "disconnected")
            r = run(a.read("printer.state"))
            self.assertEqual((r["value"], r["quality"], r["source"]), ("offline", "GOOD", "simulator"))
            self.assertEqual(kind_of(a.read("hotend.temperature")), "disconnected")
        finally:
            self.fake.closed = False
        self.assertEqual(run(a.health()), "healthy")

    def test_unreachable_server(self):
        a = adapter_for("http://127.0.0.1:1", "k", timeout_ms=500)
        # A refused connection is "disconnected" on Linux and often "timeout" on Windows,
        # where connecting to a closed port can take longer than the adapter waits.
        self.assertIn(kind_of(a.connect()), ("disconnected", "timeout"))
        self.assertEqual(run(a.health())[0], "disconnected")


# --- A real OctoPrint ----------------------------------------------------------------------

OCTOPRINT = h.octoprint_executable()


@unittest.skipUnless(OCTOPRINT, "set OPENMHS_OCTOPRINT to an octoprint executable (requirements.txt)")
class RealOctoPrint(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.octo = h.OctoPrintServer(OCTOPRINT).__enter__()
        cls.proxy = h.RecordingProxy(cls.octo.url).__enter__()

    @classmethod
    def tearDownClass(cls):
        cls.proxy.__exit__()
        cls.octo.__exit__()

    def setUp(self):
        self.proxy.mode = ("pass",)
        self.proxy.reset()
        if self.octo.connection_state() != "Operational":
            self.octo.reconnect_printer()

    def adapter(self, **kw):
        return adapter_for(self.proxy.url, self.octo.key, **kw)

    def test_version_is_the_one_the_fixtures_were_recorded_from(self):
        _, version = self.octo.request("GET", "/api/version")
        recorded = (HERE / "fixtures" / "VERSION").read_text(encoding="utf-8").strip()
        self.assertEqual(version["text"], recorded)

    def test_discovery_and_health(self):
        a = self.adapter()
        run(a.connect())
        d = run(a.describe())
        self.assertEqual(d["device"]["id"], "octoprint.virtual-01")
        self.assertEqual(sorted(c["name"] for c in d["capabilities"]), sorted(CAPABILITIES))
        self.assertEqual(run(a.health()), "healthy")

    def test_every_capability_reads_a_typed_value(self):
        a = self.adapter()
        now = time.time()
        for name in CAPABILITIES:
            with self.subTest(capability=name):
                r = run(a.read(name))
                self.assertEqual((r["quality"], r["source"]), ("GOOD", "simulator"))
                if name == "printer.state":
                    self.assertEqual(r["value"], "operational")
                else:
                    self.assertEqual(r["value"]["unit"], "Cel")
                    self.assertIsInstance(r["value"]["value"], float)
                    self.assertTrue(18 < r["value"]["value"] < 30 or "target" in name, r["value"])
                age = now - time.mktime(time.strptime(r["timestamp"][:19], "%Y-%m-%dT%H:%M:%S")) + time.timezone
                self.assertLess(abs(age), 30, r["timestamp"])

    def test_unreadable_capability_is_unsupported(self):
        a = self.adapter()
        for name in ("job.cancel", "gcode.send", "hotend.setpoint"):
            self.assertEqual(kind_of(a.read(name)), "unsupported")

    def test_a_wrong_key_is_refused_by_octoprint(self):
        a = adapter_for(self.proxy.url, "0" * 32)
        self.assertEqual(kind_of(a.read("printer.state")), "device_failure")

    def test_disconnect_and_recovery(self):
        a = self.adapter()
        self.assertEqual(run(a.read("hotend.temperature"))["quality"], "GOOD")

        self.octo.disconnect_printer()
        status, why = run(a.health())
        self.assertEqual(status, "disconnected", why)
        r = run(a.read("printer.state"))
        self.assertEqual((r["value"], r["quality"], r["source"]), ("offline", "GOOD", "simulator"))
        for name in ("hotend.temperature", "bed.temperature"):
            self.assertEqual(kind_of(a.read(name)), "disconnected", name)

        self.octo.reconnect_printer()
        self.assertEqual(run(a.health()), "healthy")
        self.assertEqual(run(a.read("printer.state"))["value"], "operational")
        self.assertEqual(run(a.read("hotend.temperature"))["quality"], "GOOD")

    def test_faults_against_the_real_server_never_change_the_printer(self):
        before = self.octo.request("GET", "/api/printer")[1]["temperature"]
        for mode, expected in FAULTS:
            a = self.adapter(timeout_ms=700)
            self.proxy.mode = mode
            self.assertEqual(kind_of(a.read("hotend.temperature")), expected, mode[0])
        self.proxy.mode = ("pass",)
        self.assertEqual(self.proxy.methods(), {"GET"})
        after = self.octo.request("GET", "/api/printer")[1]["temperature"]
        self.assertEqual(before["tool0"]["target"], after["tool0"]["target"])
        self.assertEqual(before["bed"]["target"], after["bed"]["target"])
        self.assertEqual(self.octo.connection_state(), "Operational")

    def test_the_wire_shows_only_reads(self):
        a = self.adapter()
        run(a.connect())
        for name in CAPABILITIES:
            run(a.read(name))
        run(a.health())
        self.octo.disconnect_printer()
        run(a.health())
        kind_of(a.read("hotend.temperature"))
        self.assertGreater(self.proxy.count(), 10)
        self.assertEqual(self.proxy.methods(), {"GET"})
        with self.proxy.lock:
            self.assertLessEqual({p for _, p, _ in self.proxy.requests}, set(oa.GET_PATHS))
            self.assertFalse(any("command" in p or "job" in p for _, p, _ in self.proxy.requests))

    def test_the_api_key_is_not_in_the_results(self):
        a = self.adapter()
        seen = []
        for name in CAPABILITIES:
            seen.append(json.dumps(run(a.read(name))))
        seen.append(json.dumps(run(a.describe())))
        self.octo.disconnect_printer()
        seen.append(run(a.health())[1])
        try:
            run(a.read("hotend.temperature"))
        except AdapterError as e:
            seen.append(e.message)
        self.assertFalse(any(self.octo.key in s for s in seen))


# --- Behind a real gateway ------------------------------------------------------------------

BIN = os.environ.get("OPENMHS_BIN")


@unittest.skipUnless(OCTOPRINT and BIN and Path(BIN).exists(), "set OPENMHS_OCTOPRINT and OPENMHS_BIN")
class ThroughTheGateway(unittest.TestCase):
    """The adapter as the gateway runs it: separate process, manifest permissions, grants,
    policy, the audit log."""

    DEVICE = "octoprint.virtual-01"

    @classmethod
    def setUpClass(cls):
        cls.octo = h.OctoPrintServer(OCTOPRINT).__enter__()
        cls.proxy = h.RecordingProxy(cls.octo.url).__enter__()
        cls.home = Path(tempfile.mkdtemp(prefix="openmhs-gw-"))
        manifest = json.loads(json.dumps(MANIFEST))
        manifest["runtime"]["command"] = [sys.executable, str(HERE / "octoprint_adapter.py")]
        manifest["runtime"]["env"] = {"PYTHONPATH": str(ROOT / "python")}
        manifest["endpoints"]["network"] = [cls.proxy.url]
        manifest["config"]["url"] = cls.proxy.url
        cls.manifest = cls.home / "manifest.json"
        cls.manifest.write_text(json.dumps(manifest), encoding="utf-8")
        cls.port = h.free_port()
        env = dict(os.environ, OPENMHS_HOME=str(cls.home / "gw"), OCTOPRINT_API_KEY=cls.octo.key)
        env.pop("OPENMHS_TOKEN", None)
        cls.env = env
        cls.log = open(cls.home / "gateway.log", "wb")
        cls.gateway = subprocess.Popen(
            [BIN, "serve", "--adapter-manifest", str(cls.manifest), "--allow-unsigned-adapters", "--port", str(cls.port)],
            env=env,
            stdout=cls.log,
            stderr=subprocess.STDOUT,
        )
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            if cls.cli("devices", "list", token=None)[0] == 0:
                break
            time.sleep(0.5)
        else:
            raise TimeoutError("the gateway did not start: " + (cls.home / "gateway.log").read_text(errors="replace"))
        rc, out = cls.cli("tokens", "create", "agent:demo", "--any-scope", token=None)
        cls.agent = re.search(r"omhs_[A-Za-z0-9_\-]+", out).group(0)

    @classmethod
    def tearDownClass(cls):
        cls.gateway.terminate()
        try:
            cls.gateway.wait(timeout=15)
        except subprocess.TimeoutExpired:
            cls.gateway.kill()
        cls.log.close()
        cls.proxy.__exit__()
        cls.octo.__exit__()
        shutil.rmtree(cls.home, ignore_errors=True)

    @classmethod
    def cli(cls, *args, token="agent"):
        env = dict(cls.env)
        if token == "agent":
            env["OPENMHS_TOKEN"] = cls.agent
        p = subprocess.run([BIN, *args], env=env, capture_output=True, text=True, timeout=60)
        return p.returncode, p.stdout + p.stderr

    def setUp(self):
        if self.octo.connection_state() != "Operational":
            self.octo.reconnect_printer()

    # The tests are numbered because the grants build on each other.

    def test_1_the_device_is_discovered_with_its_capabilities(self):
        rc, out = self.cli("--json", "devices", "list", token=None)
        self.assertEqual(rc, 0, out)
        devices = json.loads(out)
        d = next(x for x in devices if x["device"]["id"] == self.DEVICE)
        self.assertEqual(sorted(c["name"] for c in d["capabilities"]), sorted(CAPABILITIES))
        self.assertTrue(all(c["operations"] == ["read"] for c in d["capabilities"]))

    def test_2_no_grant_means_no_request_reaches_octoprint(self):
        before = self.proxy.count()
        rc, out = self.cli("read", self.DEVICE, "hotend.temperature", token=None)
        self.assertNotEqual(rc, 0)
        self.assertIn("policy.denied.no_grant", out)
        rc, out = self.cli("read", self.DEVICE, "hotend.temperature")
        self.assertIn("policy.denied.no_grant", out)
        self.assertEqual(self.proxy.count(), before, "a denied read must not touch OctoPrint")

    def test_3_a_granted_read_returns_octoprints_value(self):
        rc, out = self.cli(
            "grants", "add", "--id", "agent-printer-read", "--principal", "agent:demo", "--device", self.DEVICE,
            "--capability", "*", "--ops", "read", "--effect", "allow", token=None,
        )
        self.assertEqual(rc, 0, out)
        for name in CAPABILITIES:
            rc, out = self.cli("--json", "read", self.DEVICE, name)
            self.assertEqual(rc, 0, out)
            r = json.loads(out)
            self.assertEqual((r["state"]["quality"], r["state"]["source"], r["usable"]), ("GOOD", "simulator", True), name)
        rc, out = self.cli("--json", "read", self.DEVICE, "printer.state")
        self.assertEqual(json.loads(out)["state"]["value"], "operational")

    def test_4_a_write_is_refused_and_nothing_is_sent(self):
        before = self.proxy.count()
        rc, out = self.cli("exec", self.DEVICE, "hotend.target", "--value", "200Cel")
        self.assertIn("DENIED", out)
        self.assertIn("nothing sent to the device", out)
        self.assertEqual(self.proxy.count(), before)

    def test_5_even_a_write_grant_cannot_make_a_read_only_device_writable(self):
        rc, out = self.cli(
            "grants", "add", "--id", "agent-printer-write", "--principal", "agent:demo", "--device", self.DEVICE,
            "--capability", "*", "--ops", "write", "--effect", "allow", token=None,
        )
        self.assertEqual(rc, 0, out)
        before = self.proxy.count()
        rc, out = self.cli("exec", self.DEVICE, "hotend.target", "--value", "200Cel")
        self.assertIn("DENIED", out)
        self.assertNotIn("APPLIED", out)
        self.assertNotIn("VERIFIED", out)
        self.assertEqual(self.proxy.count(), before, "the descriptor declares no write; nothing may be sent")
        self.assertEqual(self.octo.request("GET", "/api/printer")[1]["temperature"]["tool0"]["target"], 0.0)
        self.assertEqual(self.proxy.methods() - {"GET"}, set())

    def test_6_a_disconnected_printer_is_an_error_not_a_stale_value(self):
        self.octo.disconnect_printer()
        rc, out = self.cli("read", self.DEVICE, "hotend.temperature")
        self.assertNotEqual(rc, 0)
        self.assertIn("adapter.disconnected", out)
        rc, out = self.cli("--json", "read", self.DEVICE, "printer.state")
        self.assertEqual(json.loads(out)["state"]["value"], "offline")
        self.octo.reconnect_printer()
        rc, out = self.cli("--json", "read", self.DEVICE, "hotend.temperature")
        self.assertEqual(rc, 0, out)
        self.assertEqual(json.loads(out)["state"]["quality"], "GOOD")

    def test_7_octoprint_unreachable_is_an_error(self):
        self.proxy.mode = ("drop",)
        try:
            rc, out = self.cli("read", self.DEVICE, "hotend.temperature")
            self.assertNotEqual(rc, 0)
            self.assertRegex(out, r"adapter\.(disconnected|timeout)")
        finally:
            self.proxy.mode = ("pass",)
        rc, out = self.cli("read", self.DEVICE, "hotend.temperature")
        self.assertEqual(rc, 0, out)

    def test_8_audit_chain_verifies_and_holds_no_secret(self):
        rc, out = self.cli("audit", "verify", token=None)
        self.assertEqual(rc, 0, out)
        self.assertEqual(self.proxy.methods() - {"GET"}, set(), "the adapter sent something other than a GET")
        blobs = [(self.home / "gateway.log").read_bytes()]
        for p in (self.home / "gw").rglob("*"):
            if p.is_file() and p.name != "gateway.lock":
                try:
                    blobs.append(p.read_bytes())
                except OSError:
                    pass
        self.assertFalse(any(self.octo.key.encode() in b for b in blobs), "the API key leaked into the gateway home or log")


if __name__ == "__main__":
    unittest.main()
