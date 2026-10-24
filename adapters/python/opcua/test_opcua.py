"""The OPC UA adapter against simulated servers: certificate checks, refusals, readings.

Run with asyncua installed (requirements.txt):
    python -m unittest discover -s adapters/python/opcua -v
Certificates are generated for each run; nothing outside loopback is contacted.
"""

import datetime as dt
import ipaddress
import json
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from cryptography import x509  # noqa: E402
from cryptography.hazmat.primitives import hashes, serialization  # noqa: E402
from cryptography.hazmat.primitives.asymmetric import rsa  # noqa: E402
from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID  # noqa: E402

import opcua_adapter as oa  # noqa: E402
from openmhs.adapter import AdapterError, Permissions  # noqa: E402

SERVER_URI = "urn:openmhs:sim:chamber-server"
CLIENT_URI = "urn:openmhs:adapter:opcua"
NOW = dt.datetime.now(dt.timezone.utc)


def key():
    return rsa.generate_private_key(public_exponent=65537, key_size=2048)


def cert(subject_key, cn, *, issuer=None, issuer_key=None, ca=False, uri=None, start=None, end=None):
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, cn)])
    b = (
        x509.CertificateBuilder()
        .subject_name(name)
        .issuer_name(issuer.subject if issuer else name)
        .public_key(subject_key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(start or NOW - dt.timedelta(days=1))
        .not_valid_after(end or NOW + dt.timedelta(days=30))
        .add_extension(x509.BasicConstraints(ca=ca, path_length=None), critical=True)
    )
    if ca:
        b = b.add_extension(
            x509.KeyUsage(True, False, False, False, False, True, True, False, False), critical=True
        )
    else:
        b = b.add_extension(
            x509.KeyUsage(True, True, True, True, False, False, False, False, False), critical=True
        )
        b = b.add_extension(
            x509.ExtendedKeyUsage([ExtendedKeyUsageOID.SERVER_AUTH, ExtendedKeyUsageOID.CLIENT_AUTH]), critical=False
        )
        b = b.add_extension(
            x509.SubjectAlternativeName(
                [
                    x509.UniformResourceIdentifier(uri),
                    x509.DNSName("localhost"),
                    x509.IPAddress(ipaddress.ip_address("127.0.0.1")),
                ]
            ),
            critical=False,
        )
    return b.sign(issuer_key or subject_key, hashes.SHA256())


class Pki:
    """ca.der trusts server.der; other-ca.der trusts other-server.der; expired.der and
    wrong-uri.der are issued by ca.der; self.der is self-signed (pinned by itself)."""

    def __init__(self, root: Path):
        self.root = root
        ca_key, other_key = key(), key()
        self.ca = cert(ca_key, "Test CA", ca=True)
        other_ca = cert(other_key, "Other CA", ca=True)
        self._write("ca", self.ca)
        self._write("other-ca", other_ca)
        server_key = key()
        self._write_key("server", server_key)
        for stem, issuer, ikey, uri, start, end in [
            ("server", self.ca, ca_key, SERVER_URI, None, None),
            ("other-server", other_ca, other_key, SERVER_URI, None, None),
            ("expired", self.ca, ca_key, SERVER_URI, NOW - dt.timedelta(days=60), NOW - dt.timedelta(days=30)),
            ("wrong-uri", self.ca, ca_key, "urn:someone:else", None, None),
            ("self", None, None, SERVER_URI, None, None),
        ]:
            self._write(stem, cert(server_key, stem, issuer=issuer, issuer_key=ikey, uri=uri, start=start, end=end))
        client_key = key()
        self._write_key("client", client_key)
        self._write("client", cert(client_key, "OpenMHS adapter", uri=CLIENT_URI))

    def _write(self, stem, c):
        (self.root / f"{stem}.der").write_bytes(c.public_bytes(serialization.Encoding.DER))

    def _write_key(self, stem, k):
        (self.root / f"{stem}.pem").write_bytes(
            k.private_bytes(
                serialization.Encoding.PEM, serialization.PrivateFormat.TraditionalOpenSSL, serialization.NoEncryption()
            )
        )

    def path(self, name):
        return str(self.root / name)


def descriptor():
    m = json.loads((HERE / "manifest.sim.json").read_text(encoding="utf-8"))
    return m["config"]["descriptor"], m["config"]["nodes"]


def config(endpoint, **extra):
    desc, nodes = descriptor()
    return {"endpoint": endpoint, "timeout_ms": 2000, "descriptor": desc, "nodes": nodes, **extra}


class SecureServer:
    def __init__(self, pki, stem):
        self.sim = oa.ChamberSim(
            "opc.tcp://127.0.0.1:0/openmhs/chamber", security=(pki.path(f"{stem}.der"), pki.path("server.pem"))
        )

    async def __aenter__(self):
        await self.sim.start()
        return self.sim

    async def __aexit__(self, *exc):
        await self.sim.stop()


class OpcUaTests(unittest.IsolatedAsyncioTestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.pki = Pki(Path(cls.tmp.name))

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def perms(self, endpoint):
        return Permissions(network=(endpoint,), filesystem_read=(self.tmp.name,))

    def security(self, trusted, **extra):
        return {
            "policy": "Basic256Sha256",
            "mode": "SignAndEncrypt",
            "certificate": self.pki.path("client.der"),
            "private_key": self.pki.path("client.pem"),
            "trusted": [self.pki.path(t) for t in trusted],
            **extra,
        }

    async def adapter(self, endpoint, **extra):
        a = oa.OpcUaAdapter()
        await a.initialize(config(endpoint, **extra), self.perms(endpoint))
        self.addAsyncCleanup(a.shutdown)
        return a

    async def refused(self, stem, trusted, expect, **extra):
        async with SecureServer(self.pki, stem) as sim:
            a = await self.adapter(sim.endpoint, security=self.security(trusted, **extra))
            with self.assertRaises(AdapterError) as e:
                await a.connect()
            self.assertEqual(e.exception.kind, "not_sent", e.exception.message)
            self.assertIn(expect, e.exception.message)
            # A refused server is never read from.
            with self.assertRaises(AdapterError):
                await a.read("chamber.temperature")

    async def test_a_server_certificate_from_a_trusted_ca_connects(self):
        async with SecureServer(self.pki, "server") as sim:
            a = await self.adapter(sim.endpoint, security=self.security(["ca.der"], server_uri=SERVER_URI))
            r = await a.read("chamber.temperature")
            self.assertEqual(r["quality"], "GOOD")
            self.assertEqual(r["value"]["unit"], "Cel")
            self.assertEqual(await a.health(), "healthy")

    async def test_a_pinned_self_signed_certificate_connects(self):
        async with SecureServer(self.pki, "self") as sim:
            a = await self.adapter(sim.endpoint, security=self.security(["self.der"]))
            self.assertIs((await a.read("door.closed"))["value"], True)

    async def test_a_certificate_from_an_untrusted_ca_is_refused(self):
        await self.refused("other-server", ["ca.der"], "is not trusted")

    async def test_an_expired_certificate_is_refused(self):
        await self.refused("expired", ["ca.der"], "expired")

    async def test_a_certificate_for_another_application_is_refused(self):
        await self.refused("wrong-uri", ["ca.der"], "not " + SERVER_URI, server_uri=SERVER_URI)

    async def test_plaintext_beyond_loopback_needs_allow_plaintext(self):
        a = oa.OpcUaAdapter()
        with self.assertRaises(AdapterError) as e:
            await a.initialize(config("opc.tcp://192.0.2.10:4840"), self.perms("opc.tcp://192.0.2.10:4840"))
        self.assertIn("allow_plaintext", e.exception.message)
        # Accepted when asked for; nothing is contacted until connect.
        await a.initialize(
            config("opc.tcp://192.0.2.10:4840", allow_plaintext=True), self.perms("opc.tcp://192.0.2.10:4840")
        )

    async def test_endpoints_and_files_outside_the_manifest_are_refused(self):
        a = oa.OpcUaAdapter()
        with self.assertRaises(AdapterError) as e:
            await a.initialize(config("opc.tcp://127.0.0.1:4840"), Permissions(network=("opc.tcp://127.0.0.1:4841",)))
        self.assertEqual(e.exception.kind, "not_sent")
        with self.assertRaises(AdapterError) as e:
            await a.initialize(
                config("opc.tcp://127.0.0.1:4840", security=self.security(["ca.der"])),
                Permissions(network=("opc.tcp://127.0.0.1:4840",)),
            )
        self.assertIn("filesystem.read", e.exception.message)

    async def test_configuration_mistakes_are_refused(self):
        desc, nodes = descriptor()
        cases = {
            "verification cannot be disabled": dict(security={"policy": "Basic256Sha256", "insecure": True}),
            "a namespace index is refused": dict(nodes={**nodes, "chamber.mode": "ns=2;s=Chamber.Mode"}),
            "a node for every readable property": dict(nodes={k: v for k, v in nodes.items() if k != "door.closed"}),
            "certificates are required": dict(security={"policy": "Basic256Sha256"}),
        }
        for what, extra in cases.items():
            with self.subTest(what):
                with self.assertRaises(AdapterError):
                    await oa.OpcUaAdapter().initialize(
                        config("opc.tcp://127.0.0.1:4840", **extra), self.perms("opc.tcp://127.0.0.1:4840")
                    )
        writable = json.loads(json.dumps(desc))
        writable["capabilities"][0]["operations"] = ["read", "write"]
        with self.assertRaises(AdapterError) as e:
            await oa.OpcUaAdapter().initialize(
                {**config("opc.tcp://127.0.0.1:4840"), "descriptor": writable}, self.perms("opc.tcp://127.0.0.1:4840")
            )
        self.assertIn("read-only", e.exception.message)

    async def test_a_lost_server_surfaces_as_disconnected_and_comes_back(self):
        sim = oa.ChamberSim("opc.tcp://127.0.0.1:0/openmhs/chamber")
        await sim.start()
        try:
            a = await self.adapter(sim.endpoint)
            first = await a.read("chamber.humidity")
            self.assertEqual(first["value"]["unit"], "%")
            await sim.stop()
            with self.assertRaises(AdapterError) as e:
                await a.read("chamber.humidity")
            self.assertIn(e.exception.kind, ("disconnected", "timeout"))
            self.assertNotEqual(await a.health(), "healthy")
            await sim.start()
            self.assertEqual((await a.read("chamber.mode"))["value"], "running")
        finally:
            await sim.stop()


class ConversionTests(unittest.TestCase):
    def test_values_follow_the_descriptor(self):
        num = {"type": "number", "unit": "Cel"}
        self.assertEqual(oa.convert(21, num, None, "n"), {"value": 21.0, "unit": "Cel"})
        for bad in (True, "21", float("nan"), None):
            with self.assertRaises(AdapterError):
                oa.convert(bad, num, None, "n")
        enum = {"type": "enum", "values": ["idle", "running"]}
        self.assertEqual(oa.convert(1, enum, ["idle", "running"], "n"), "running")
        with self.assertRaises(AdapterError):
            oa.convert(5, enum, ["idle", "running"], "n")

    def test_node_ids_name_their_namespace(self):
        self.assertEqual(oa.parse_node("nsu=urn:a:b;s=X.Y"), ("urn:a:b", "s=X.Y"))
        self.assertEqual(oa.parse_node("i=2259"), (None, "i=2259"))
        with self.assertRaises(oa.ConfigError):
            oa.parse_node("ns=3;s=X")


if __name__ == "__main__":
    unittest.main()
