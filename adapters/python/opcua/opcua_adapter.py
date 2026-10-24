"""A read-only OPC UA adapter (docs/adapters/opcua.md).

The manifest's ``config`` holds the device descriptor, the endpoint, the security
settings and one OPC UA node per readable capability. Every ``read`` is a direct Read
service call: the value, its StatusCode and its timestamps come from the server, never
from a cache. Writes and method calls are not supported in this release; a descriptor
that declares them is refused.

Node ids name their namespace by URI (``nsu=urn:acme:line1;s=Oven.Temperature``),
resolved to the server's current index on every connect: namespace indexes may change
when a server restarts, URIs do not.

Run ``python opcua_adapter.py browse --manifest manifest.json`` to list a server's
variables while writing a profile; it applies the manifest's security settings.

Requires asyncua (adapters/python/opcua/requirements.txt). Licence: LGPL-3.0-or-later,
installed separately with pip; nothing of it is bundled here.
"""

from __future__ import annotations

import argparse
import asyncio
import ipaddress
import json
import math
import os
import random
import socket
import sys
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "python"))

from asyncua import Client, Server, ua  # noqa: E402
from asyncua.crypto import security_policies  # noqa: E402
from asyncua.crypto import uacrypto  # noqa: E402
from cryptography import x509  # noqa: E402
from cryptography.exceptions import InvalidSignature  # noqa: E402
from cryptography.hazmat.primitives.serialization import Encoding  # noqa: E402

from openmhs.adapter import Adapter, AdapterError, Permissions, reading, run  # noqa: E402

READ_ONLY = {"read", "subscribe"}

POLICIES = {
    "Basic256Sha256": security_policies.SecurityPolicyBasic256Sha256,
    "Aes128_Sha256_RsaOaep": security_policies.SecurityPolicyAes128Sha256RsaOaep,
    "Aes256_Sha256_RsaPss": security_policies.SecurityPolicyAes256Sha256RsaPss,
}
MODES = {
    "Sign": ua.MessageSecurityMode.Sign,
    "SignAndEncrypt": ua.MessageSecurityMode.SignAndEncrypt,
}

#: Bad StatusCodes that mean the server has lost the device, not that the value is wrong.
LINK_LOST = {
    "BadCommunicationError",
    "BadNotConnected",
    "BadNoCommunication",
    "BadServerNotConnected",
    "BadWaitingForInitialData",
    "BadConnectionClosed",
    "BadSecureChannelClosed",
    "BadSessionClosed",
    "BadSessionIdInvalid",
}

SERVER_STATE = ua.NodeId(2259, 0)  # Server.ServerStatus.State


class ConfigError(Exception):
    pass


def is_loopback(host: str) -> bool:
    if host.lower() == "localhost":
        return True
    try:
        return ipaddress.ip_address(host.strip("[]")).is_loopback
    except ValueError:
        return False


def parse_node(text: str) -> tuple:
    """``(namespace_uri | None, rest)`` for ``nsu=<uri>;<id>`` or a namespace-0 id."""
    if text.startswith("nsu="):
        uri, sep, rest = text[4:].partition(";")
        if not sep or not uri or not rest:
            raise ConfigError(f"node {text!r}: expected nsu=<namespace URI>;<identifier>")
        return uri, rest
    if text.startswith("ns="):
        index, _, rest = text[3:].partition(";")
        if index != "0":
            raise ConfigError(
                f"node {text!r}: name the namespace by URI (nsu=<uri>;{rest}); "
                "namespace indexes can change when the server restarts"
            )
        return None, rest
    return None, text


def rfc3339(dt) -> str | None:
    if dt is None:
        return None
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.astimezone(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


# --- Certificates ---------------------------------------------------------------------


def load_cert(path: Path) -> x509.Certificate:
    data = path.read_bytes()
    if b"-----BEGIN" in data:
        return x509.load_pem_x509_certificate(data)
    return x509.load_der_x509_certificate(data)


def load_trusted(paths: list) -> list:
    """Certificates in each file, or in each ``.der``/``.pem``/``.crt`` file of a directory."""
    certs = []
    for p in map(Path, paths):
        files = sorted(f for f in p.iterdir() if f.suffix.lower() in (".der", ".pem", ".crt")) if p.is_dir() else [p]
        for f in files:
            try:
                certs.append(load_cert(f))
            except (OSError, ValueError) as e:
                raise ConfigError(f"trusted certificate {f}: {e}") from e
    if not certs:
        raise ConfigError(f"security.trusted lists no certificates ({paths})")
    return certs


def check_server_certificate(cert: x509.Certificate, trusted: list, server_uri: str | None, now=None) -> None:
    """Raise unless ``cert`` is trusted: it is one of ``trusted`` (pinned), or a trusted CA
    issued it directly. Its validity period must include ``now``, and when ``server_uri``
    is set its subjectAltName must carry that URI."""
    now = now or datetime.now(timezone.utc)
    subject = cert.subject.rfc4514_string()
    if not cert.not_valid_before_utc <= now <= cert.not_valid_after_utc:
        raise AdapterError(
            "not_sent",
            f"server certificate {subject} is expired or not yet valid "
            f"(valid {rfc3339(cert.not_valid_before_utc)} to {rfc3339(cert.not_valid_after_utc)})",
        )
    der = cert.public_bytes(Encoding.DER)
    pinned = any(t.public_bytes(Encoding.DER) == der for t in trusted)
    if not pinned:
        issuers = [t for t in trusted if _is_ca(t) and t.subject == cert.issuer]
        if not any(_issued_by(cert, ca, now) for ca in issuers):
            raise AdapterError(
                "not_sent",
                f"server certificate {subject} is not trusted: it is neither in security.trusted "
                f"nor issued by a trusted CA (issuer {cert.issuer.rfc4514_string()})",
            )
    if server_uri is not None:
        try:
            uris = cert.extensions.get_extension_for_class(x509.SubjectAlternativeName).value.get_values_for_type(
                x509.UniformResourceIdentifier
            )
        except x509.ExtensionNotFound:
            uris = []
        if server_uri not in uris:
            raise AdapterError(
                "not_sent", f"server certificate is for {uris or 'no application URI'}, not {server_uri}"
            )


def _is_ca(cert: x509.Certificate) -> bool:
    try:
        return cert.extensions.get_extension_for_class(x509.BasicConstraints).value.ca
    except x509.ExtensionNotFound:
        return False


def _issued_by(cert: x509.Certificate, ca: x509.Certificate, now) -> bool:
    if not ca.not_valid_before_utc <= now <= ca.not_valid_after_utc:
        return False
    try:
        cert.verify_directly_issued_by(ca)
        return True
    except (InvalidSignature, ValueError, TypeError):
        return False


# --- Configuration --------------------------------------------------------------------


class Settings:
    """The manifest's ``config``, checked."""

    def __init__(self, config: dict, permissions: Permissions):
        known = {"endpoint", "security", "allow_plaintext", "user", "timeout_ms", "descriptor", "nodes", "simulate"}
        unknown = set(config) - known
        if unknown:
            raise ConfigError(f"unknown config keys {sorted(unknown)}")
        self.endpoint = config.get("endpoint")
        if not isinstance(self.endpoint, str) or not self.endpoint.startswith("opc.tcp://"):
            raise ConfigError("config.endpoint must be an opc.tcp:// URL")
        self.host = urlsplit(self.endpoint).hostname or ""
        self.simulate = bool(config.get("simulate", False))
        if self.simulate and not is_loopback(self.host):
            raise ConfigError("config.simulate needs a loopback endpoint")
        self.timeout = float(config.get("timeout_ms", 2000)) / 1000
        self.descriptor = config.get("descriptor")
        if not isinstance(self.descriptor, dict):
            raise ConfigError("config.descriptor must be the device descriptor")
        self.nodes = self._nodes(config.get("nodes"), self.descriptor)

        sec = dict(config.get("security") or {"policy": "None"})
        unknown = set(sec) - {"policy", "mode", "certificate", "private_key", "trusted", "server_uri"}
        if unknown:
            raise ConfigError(f"unknown security keys {sorted(unknown)}")
        self.policy = sec.get("policy", "None")
        if self.policy == "None":
            if set(sec) - {"policy"}:
                raise ConfigError("security.policy None takes no certificates")
            if not is_loopback(self.host) and not config.get("allow_plaintext", False):
                raise ConfigError(
                    f"refusing an unsigned, unencrypted OPC UA connection to {self.host}: set a "
                    "security.policy, or set allow_plaintext to accept it (docs/security/transport-security.md)"
                )
        else:
            if self.policy not in POLICIES:
                raise ConfigError(f"security.policy must be None or one of {sorted(POLICIES)}")
            self.mode = sec.get("mode", "SignAndEncrypt")
            if self.mode not in MODES:
                raise ConfigError(f"security.mode must be one of {sorted(MODES)}")
            for key in ("certificate", "private_key", "trusted"):
                if not sec.get(key):
                    raise ConfigError(f"security.policy {self.policy} needs security.{key}")
            self.certificate = permissions.check_read(sec["certificate"])
            self.private_key = permissions.check_read(sec["private_key"])
            trusted = sec["trusted"] if isinstance(sec["trusted"], list) else [sec["trusted"]]
            self.trusted = load_trusted([permissions.check_read(t) for t in trusted])
            self.server_uri = sec.get("server_uri")

        user = config.get("user")
        self.username = self.password = None
        if user is not None:
            for key in ("username_env", "password_env"):
                name = user.get(key)
                if not name or name not in os.environ:
                    raise ConfigError(f"user.{key} must name a credential the manifest passes ({name!r} is unset)")
            self.username = os.environ[user["username_env"]]
            self.password = os.environ[user["password_env"]]

    @staticmethod
    def _nodes(nodes, descriptor) -> dict:
        if not isinstance(nodes, dict):
            raise ConfigError("config.nodes must map capability names to OPC UA node ids")
        caps = {c.get("name"): c for c in descriptor.get("capabilities", [])}
        out = {}
        for cap in caps.values():
            ops = set(cap.get("operations", []))
            if ops - READ_ONLY:
                raise ConfigError(
                    f"{cap['name']}: operations {sorted(ops - READ_ONLY)} are not supported; "
                    "the OPC UA adapter is read-only in this release"
                )
            if cap.get("kind") == "property" and "read" in ops and cap["name"] not in nodes:
                raise ConfigError(f"{cap['name']} is readable but config.nodes has no node for it")
        for name, spec in nodes.items():
            if name not in caps:
                raise ConfigError(f"config.nodes.{name}: no such capability in the descriptor")
            spec = {"node": spec} if isinstance(spec, str) else dict(spec)
            if set(spec) - {"node", "enum"} or not isinstance(spec.get("node"), str):
                raise ConfigError(f"config.nodes.{name}: expected a node id or {{node, enum}}")
            spec["parsed"] = parse_node(spec["node"])
            out[name] = spec
        return out


# --- Values ---------------------------------------------------------------------------


def convert(value, spec: dict, enum: list | None, where: str):
    """An OPC UA value as the descriptor's value spec says, or ``malformed_response``."""

    def bad(why):
        return AdapterError("malformed_response", f"{where}: {why} (got {value!r})")

    kind = spec.get("type")
    if kind == "number":
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
            raise bad("expected a finite number")
        return {"value": float(value), "unit": spec["unit"]} if "unit" in spec else float(value)
    if kind == "integer":
        if isinstance(value, bool) or not isinstance(value, int):
            raise bad("expected an integer")
        return value
    if kind == "boolean":
        if not isinstance(value, bool):
            raise bad("expected a Boolean")
        return value
    if kind == "string":
        if not isinstance(value, str):
            raise bad("expected a String")
        return value
    if kind == "enum":
        if enum is not None and isinstance(value, int) and not isinstance(value, bool):
            if not 0 <= value < len(enum):
                raise bad(f"no enum entry for {value}")
            value = enum[value]
        if value not in spec.get("values", []):
            raise bad(f"expected one of {spec.get('values')}")
        return value
    raise bad(f"value type {kind!r} is not supported")


# --- The adapter ----------------------------------------------------------------------


class OpcUaAdapter(Adapter):
    adapter_id = "openmhs.py.opcua"
    version = "0.1.0"

    def __init__(self):
        self.settings: Settings | None = None
        self.client: Client | None = None
        self.indexes: dict = {}
        self.sim: ChamberSim | None = None
        self.lock = asyncio.Lock()

    async def initialize(self, config, permissions):
        await super().initialize(config, permissions)
        try:
            self.settings = Settings(config, permissions)
        except ConfigError as e:
            raise AdapterError("internal", f"config: {e}") from e
        s = self.settings
        if s.simulate:
            self.sim = ChamberSim(s.endpoint)
            await self.sim.start()
            s.endpoint = self.sim.endpoint
            self.fixture_faults = ("link_down",)
        else:
            permissions.check_network(s.endpoint)

    async def describe(self):
        return self.settings.descriptor

    async def connect(self):
        await self._client()

    async def _client(self) -> Client:
        """The connected client, connecting first if there is none."""
        async with self.lock:
            if self.client is None:
                self.client, self.indexes = await self._open()
            return self.client

    async def _open(self):
        s = self.settings
        client = Client(s.endpoint, timeout=s.timeout)
        client.name = client.description = "OpenMHS OPC UA adapter"
        client.application_uri = "urn:openmhs:adapter:opcua"
        if s.username is not None:
            client.set_user(s.username)
            client.set_password(s.password)
        try:
            if s.policy != "None":
                await self._secure(client)
            await asyncio.wait_for(client.connect(), s.timeout * 2)
        except AdapterError:
            raise
        except ua.UaStatusCodeError as e:
            raise AdapterError("disconnected", f"connect to {s.endpoint}: {type(e).__name__}: {e}") from e
        except (OSError, asyncio.TimeoutError, ua.UaError) as e:
            raise AdapterError("disconnected", f"connect to {s.endpoint}: {type(e).__name__}: {e}") from e
        try:
            indexes = {}
            for spec in s.nodes.values():
                uri = spec["parsed"][0]
                if uri is not None and uri not in indexes:
                    try:
                        indexes[uri] = await client.get_namespace_index(uri)
                    except ValueError as e:
                        raise AdapterError("device_failure", f"the server has no namespace {uri}") from e
            return client, indexes
        except BaseException:
            await self._close(client)
            raise

    async def _secure(self, client: Client) -> None:
        """Validate the server's certificate ourselves, then pin it for the secure channel."""
        s = self.settings
        policy = POLICIES[s.policy]
        endpoints = await asyncio.wait_for(client.connect_and_get_server_endpoints(), s.timeout * 2)
        try:
            endpoint = Client.find_endpoint(endpoints, MODES[s.mode], policy.URI)
        except ua.UaError as e:
            raise AdapterError("not_sent", f"the server offers no {s.policy}/{s.mode} endpoint") from e
        der = endpoint.ServerCertificate
        if not der:
            raise AdapterError("not_sent", "the server sent no certificate for a secure endpoint")
        cert = uacrypto.x509_from_der(der)  # the leaf, if the server sent a chain
        check_server_certificate(cert, s.trusted, s.server_uri)
        await client.set_security(
            policy,
            s.certificate,
            s.private_key,
            server_certificate=cert.public_bytes(Encoding.DER),
            mode=MODES[s.mode],
        )

    async def _close(self, client: Client | None) -> None:
        if client is None:
            return
        try:
            await asyncio.wait_for(client.disconnect(), 1)
        except Exception:
            pass

    async def _drop(self) -> None:
        async with self.lock:
            client, self.client = self.client, None
        await self._close(client)

    async def _read_node(self, node_id: ua.NodeId) -> ua.DataValue:
        client = await self._client()
        try:
            return await asyncio.wait_for(client.get_node(node_id).read_data_value(raise_on_bad_status=False),
                                          self.settings.timeout)
        except asyncio.TimeoutError as e:
            await self._drop()
            raise AdapterError("timeout", "the server did not answer in time") from e
        except (OSError, ConnectionError, ua.UaError) as e:
            await self._drop()
            raise AdapterError("disconnected", f"{type(e).__name__}: {e}") from e

    async def health(self):
        try:
            dv = await self._read_node(SERVER_STATE)
        except AdapterError as e:
            return ("disconnected", e.message)
        state = dv.Value.Value if dv.Value is not None else None
        if dv.StatusCode.is_good() and state == ua.ServerState.Running:
            return "healthy"
        return ("degraded", f"server state {state!r}, status {dv.StatusCode.name}")

    async def read(self, capability):
        s = self.settings
        spec = s.nodes.get(capability)
        cap = next((c for c in s.descriptor.get("capabilities", []) if c.get("name") == capability), None)
        if spec is None or cap is None or "read" not in cap.get("operations", []):
            raise AdapterError("unsupported", f"no readable capability {capability!r}")
        await self._client()
        uri, rest = spec["parsed"]
        index = self.indexes[uri] if uri is not None else 0
        node_id = ua.NodeId.from_string(f"ns={index};{rest}")
        dv = await self._read_node(node_id)
        status = dv.StatusCode
        if status.is_bad():
            kind = "disconnected" if status.name in LINK_LOST else "device_failure"
            raise AdapterError(kind, f"{spec['node']}: {status.name}")
        value = convert(dv.Value.Value if dv.Value is not None else None, cap["value"], spec.get("enum"), spec["node"])
        return reading(
            value,
            timestamp=rfc3339(dv.SourceTimestamp or dv.ServerTimestamp),
            source="simulator" if s.simulate else "device",
            quality="UNCERTAIN" if status.is_uncertain() else "GOOD",
        )

    async def execute(self, command, progress):
        raise AdapterError("unsupported", "the OPC UA adapter is read-only in this release")

    async def fixture_inject(self, fault):
        if fault != "link_down" or self.sim is None:
            raise AdapterError("unsupported", f"cannot inject {fault}")
        await self.sim.stop()

    async def fixture_clear(self):
        if self.sim is not None:
            await self.sim.start()

    async def shutdown(self):
        await self._drop()
        if self.sim is not None:
            await self.sim.stop()


# --- Simulated server -----------------------------------------------------------------

SIM_NAMESPACE = "urn:openmhs:sim:chamber"


class ChamberSim:
    """A small OPC UA server on loopback: a climate chamber with four variables. Used by
    conformance runs (``config.simulate``) and the tests; never by a real deployment."""

    def __init__(self, endpoint: str, seed: int = 1, security=None):
        parts = urlsplit(endpoint)
        port = parts.port or 0
        if port == 0:
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0))
                port = sock.getsockname()[1]
        self.endpoint = f"opc.tcp://127.0.0.1:{port}{parts.path or '/'}"
        self.random = random.Random(seed)
        self.security = security  # (certificate path, private key path) or None
        self.server: Server | None = None
        self.task: asyncio.Task | None = None
        self.stale = False

    async def start(self) -> None:
        if self.server is not None:
            return
        server = Server()
        await server.init()
        server.set_endpoint(self.endpoint)
        server.set_server_name("OpenMHS simulated chamber")
        if self.security is None:
            server.set_security_policy([ua.SecurityPolicyType.NoSecurity])
        else:
            server.set_security_policy([ua.SecurityPolicyType.Basic256Sha256_SignAndEncrypt])
            await server.load_certificate(self.security[0])
            await server.load_private_key(self.security[1])
            await server.set_application_uri("urn:openmhs:sim:chamber-server")
        idx = await server.register_namespace(SIM_NAMESPACE)
        chamber = await server.nodes.objects.add_object(ua.NodeId("Chamber", idx), "Chamber")
        self.vars = {}
        for name, initial, vtype in [
            ("Temperature", 21.0, ua.VariantType.Double),
            ("Humidity", 45.0, ua.VariantType.Double),
            ("DoorClosed", True, ua.VariantType.Boolean),
            ("Mode", 1, ua.VariantType.Int32),
        ]:
            self.vars[name] = await chamber.add_variable(ua.NodeId(f"Chamber.{name}", idx), name, ua.Variant(initial, vtype))
        await server.start()
        self.server = server
        self.task = asyncio.create_task(self._update())

    async def _update(self) -> None:
        t = 0.0
        while True:
            t += 0.2
            now = datetime.now(timezone.utc)
            temp = 21.0 + 2.0 * math.sin(t / 5) + self.random.uniform(-0.05, 0.05)
            hum = 45.0 + self.random.uniform(-0.5, 0.5)
            for name, value, vtype in [
                ("Temperature", temp, ua.VariantType.Double),
                ("Humidity", hum, ua.VariantType.Double),
            ]:
                await self.vars[name].write_value(
                    ua.DataValue(ua.Variant(value, vtype), SourceTimestamp=now, ServerTimestamp=now)
                )
            await asyncio.sleep(0.2)

    async def stop(self) -> None:
        server, self.server = self.server, None
        if self.task is not None:
            self.task.cancel()
            self.task = None
        if server is not None:
            await server.stop()


# --- browse ---------------------------------------------------------------------------


async def browse(manifest_path: str, depth: int) -> int:
    manifest_path = os.path.abspath(manifest_path)
    manifest = json.loads(Path(manifest_path).read_text(encoding="utf-8"))
    os.chdir(os.path.dirname(manifest_path))
    perms = Permissions(
        network=tuple(manifest.get("endpoints", {}).get("network", ())),
        filesystem_read=tuple(manifest.get("filesystem", {}).get("read", ())),
    )
    adapter = OpcUaAdapter()
    try:
        await adapter.initialize(manifest.get("config", {}), perms)
        client = await adapter._client()
    except AdapterError as e:
        print(f"error: {e.message}", file=sys.stderr)
        return 1
    try:
        uris = await client.get_namespace_array()

        async def walk(node, level):
            for child in await node.get_children():
                nid = child.nodeid
                text = nid.to_string().split(";", 1)[-1] if nid.NamespaceIndex else nid.to_string()
                name = f"nsu={uris[nid.NamespaceIndex]};{text}" if nid.NamespaceIndex else text
                cls = await child.read_node_class()
                line = f"{'  ' * level}{(await child.read_browse_name()).Name}  {name}"
                if cls == ua.NodeClass.Variable:
                    dv = await child.read_data_value(raise_on_bad_status=False)
                    line += f"  = {dv.Value.Value!r} [{dv.StatusCode.name}]"
                print(line)
                if level + 1 < depth and cls == ua.NodeClass.Object:
                    await walk(child, level + 1)

        await walk(client.nodes.objects, 0)
    finally:
        await adapter.shutdown()
    return 0


def main() -> None:
    if len(sys.argv) > 1 and sys.argv[1] == "browse":
        p = argparse.ArgumentParser(prog="opcua_adapter.py browse")
        p.add_argument("--manifest", required=True)
        p.add_argument("--depth", type=int, default=3)
        a = p.parse_args(sys.argv[2:])
        sys.exit(asyncio.run(browse(a.manifest, a.depth)))
    run(OpcUaAdapter())


if __name__ == "__main__":
    main()
