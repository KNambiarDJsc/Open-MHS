# Adapter permission manifest (v0.1)

Schema: [`schemas/adapter-manifest.schema.json`](schemas/adapter-manifest.schema.json)
(`urn:openmhs:schema:0.1:adapter-manifest`). Reference parser: `openmhs_host::Manifest`.
Protocol: [adapter-protocol.md](adapter-protocol.md). Decision record:
[ADR-0015](../docs/adr/0015-adapter-host.md).

A manifest is one JSON file next to an out-of-process adapter. It says how to start the
adapter and what the adapter may touch. Check one with
`openmhs adapter check <manifest.json>`; run one with
`openmhs serve --adapter-manifest <manifest.json>`.

## What a manifest guarantees, and what it does not

Each field is one of:

- **Host-enforced**: the gateway enforces it from outside the adapter process. A
  misbehaving or compromised adapter cannot get around it.
- **Cooperative**: the gateway passes it to the adapter, and the adapter SDK checks it
  before opening anything. An adapter that skips the SDK, or is compromised, is not
  stopped. These fields document intent and catch mistakes; they are **not** a sandbox.
- **OS-enforced**: the operating system enforces it. In v0.1 the gateway applies **no**
  OS-level restriction of its own: no seccomp, Landlock, namespaces, AppArmor, SELinux,
  AppContainer or job-object limits. A manifest whose `runtime.command` starts the adapter
  in a container (as `adapters/python/ros2/manifest.sim.json` does with
  `docker run --network none --read-only`) gets whatever that container runtime enforces;
  that is the command's doing, not the manifest fields'.

| Field | Enforcement | What happens |
|---|---|---|
| `capabilities.allow` | Host | The descriptor may declare only allowed capabilities and operations; otherwise the adapter is quarantined at start. Calls outside the list are refused by the host before anything is sent to the process. |
| `runtime.env` | Host | The process gets a cleared environment plus these variables and a small OS allow-list (adapter-protocol.md §6). |
| `credentials` | Host | Only the named gateway variables are passed; their values are redacted from the adapter's logs. A missing one stops the adapter from starting. |
| `limits`, `restart` | Host | Message size, call deadlines, heartbeats, restart budget and quarantine (adapter-protocol.md §5). |
| `runtime.protocol` | Host | Versions the host may offer. |
| `endpoints.network` | Cooperative | The SDK's `permissions.check_network(url)` refuses other scheme/host/port combinations with `adapter.not_sent`. |
| `endpoints.serial` | Cooperative | `permissions.check_serial(port)`. |
| `filesystem.read`, `filesystem.write` | Cooperative | `permissions.check_read(path)` refuses paths outside the listed files and directories. Relative paths are relative to the working directory. |
| `config` | — | Passed to the adapter untouched. |

The adapter process runs as the same OS user as the gateway, with that user's file and
network access. Run adapters you do not trust in a container or a separate account, and
treat the cooperative fields as documentation of what they should need.

### Platform notes

- **Linux.** Planned (not in v0.1): Landlock for `filesystem`, a network namespace or
  seccomp filter for `endpoints`, and `prctl(PR_SET_NO_NEW_PRIVS)`. Until then, use a
  container runtime in `runtime.command`.
- **Windows.** Planned (not in v0.1): a job object (kill-on-close, process limits) and an
  AppContainer or restricted token. Until then the adapter has the gateway user's rights.
  The host's kill-on-drop ends the direct child; grandchildren the adapter starts are not
  tracked.
- **macOS.** No OS restriction is planned yet.
- **Containers.** When `runtime.command` is `docker run …`, the host supervises the
  `docker` client, not the container. A client killed for a missed heartbeat closes the
  container's stdin, and an SDK adapter exits on end of input; an adapter hung so badly
  that it never reads stdin again leaves its container running until `docker rm -f`.
  Name containers (`--name`) if you need to find them.

## Signatures

A manifest may carry a `signature`. The host verifies it **before launching anything**, and
by default refuses a manifest without one (`manifest.unsigned`). `openmhs serve
--allow-unsigned-adapters` turns that off for development and prints a warning.

```json
"signature": {
  "algorithm": "ed25519",
  "key_id": "<sha256 of the public key, hex>",
  "artifacts": [ { "path": "balance.py", "sha256": "<hex>" } ],
  "value": "<128 hex digits>"
}
```

- **What is signed.** The ASCII domain tag `openmhs-adapter-manifest-v1\n` followed by the
  canonical JSON (sorted keys, no whitespace) of the manifest with `signature.value`
  emptied. Everything is covered: the adapter id and version, the launch command,
  environment, capabilities, endpoints, filesystem entries, configuration and the digest of
  every listed file. Changing any of it invalidates the signature.
- **Which files.** `artifacts` lists the adapter's files (paths relative to the manifest,
  inside the package; directories are expanded). Every file the launch command runs from
  the package must be listed (`artifact.uncovered` otherwise). Links, missing files and
  non-regular files are refused.
- **Trusted signers.** The key must be in the operator's `trusted_keys.json`
  (`openmhs trust add/list/revoke`), not revoked, and allowed for this adapter id
  (`signer.unknown`, `signer.revoked`, `signer.not_allowed`). The registry is not
  changeable through the API.
- **No gap between check and use.** The host hashes the bytes it reads, compares them to
  the signed digests, copies them to a private staging directory, and launches (and
  restarts) from the copies. Replacing the originals afterwards changes nothing.
- **A signature is not permission.** It says who vouches for these files. The manifest's
  limits are enforced exactly as for an unsigned one: capabilities are checked against the
  descriptor at every start, and device policy and credential scopes decide what any
  caller may do. A trusted signature never widens what an adapter may do.
- **Not covered.** The interpreter and libraries the adapter uses, and files it reads at
  run time. Pin what matters in the package.

Make a key with `openmhs adapter keygen <out>` (writes `<out>.key`, secret, and
`<out>.pub`), sign with `openmhs adapter sign <manifest> --key <out>.key --artifact FILE…`,
and trust it with `openmhs trust add <out>.pub --name "…"`.

## Fields

```json
{
  "openmhs_manifest": "0.1",
  "adapter": { "id": "example.lab-balance", "version": "0.3.1" },
  "runtime": {
    "command": ["python", "-I", "{manifest_dir}/balance.py", "--port", "/dev/ttyUSB0"],
    "cwd": "{manifest_dir}",
    "env": { "BALANCE_UNITS": "g" },
    "protocol": ["0.1"]
  },
  "capabilities": { "allow": [
    { "name": "mass.*", "operations": ["read", "subscribe"] },
    { "name": "tare", "operations": ["invoke"] } ] },
  "endpoints": { "serial": ["/dev/ttyUSB0"], "network": ["https://lims.example.lab"] },
  "filesystem": { "read": ["calibration.json"], "write": [] },
  "credentials": ["LIMS_TOKEN"],
  "config": { "baud": 9600 },
  "limits": { "call_timeout_ms": 3000 },
  "restart": { "max_restarts": 3, "window_s": 120 }
}
```

(`spec/examples/manifests/lab-balance.json`.)

- `openmhs_manifest`: `"0.1"`.
- `adapter.id`: must equal the `adapter.id` in `initialize` and in the descriptor.
- `runtime.command`: program and arguments, run directly (no shell). `{manifest_dir}`
  expands to the manifest's directory in `command`, `cwd` and `env` values only. A
  program name without a path separator is looked up on `PATH`; a relative path is
  relative to the manifest.
- `runtime.cwd`: working directory; default the manifest's directory.
- `runtime.env`: non-secret variables. Secrets go in `credentials`.
- `runtime.protocol`: adapter-protocol versions; default `["0.1"]`.
- `capabilities.allow`: `name` may use `*` for any run of characters.
- `endpoints.network`: URLs; scheme, host and port are compared.
- `limits`: `max_message_bytes` (default 1048576), `start_timeout_ms` (30000),
  `call_timeout_ms` (5000), `heartbeat_ms` (2000), `missed_heartbeats` (3),
  `shutdown_grace_ms` (2000).
- `restart`: `max_restarts` (5) within `window_s` (60); backoff from
  `initial_backoff_ms` (200) doubling to `max_backoff_ms` (10000).

Unknown fields are errors, so a misspelt restriction is never silently ignored.
