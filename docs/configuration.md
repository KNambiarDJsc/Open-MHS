# Configuration

OpenMHS has no configuration file for the gateway itself. It is configured with command
line flags, environment variables, and the state it keeps in its home directory.

## The gateway home

All gateway state lives in one directory, chosen by `--home`, else `$OPENMHS_HOME`, else
the per-user local data directory plus `openmhs` (`%LOCALAPPDATA%\openmhs` on Windows,
`~/Library/Application Support/openmhs` on macOS, `$XDG_DATA_HOME/openmhs` or
`~/.local/share/openmhs` on Linux).

| File | What it is |
|---|---|
| `gateway.json` | The runtime file: gateway URL, process id, and the **operator token**. Clients find a running gateway through it |
| `gateway.lock` | Keeps two gateways from using one home |
| `journal.sqlite` (+ `-wal`, `-shm`) | The command journal, audit log, credentials, approvals, leases and plans |
| `policy.json` | Grants |
| `trusted_keys.json` | Signing keys trusted to vouch for adapters (`openmhs trust`) |

**Anything that can read your home can act as the operator.** Treat it like an SSH
directory. Give agents their own scoped tokens; never the contents of `gateway.json`.

## Running the gateway

`openmhs serve` (or `openmhsd`) takes:

| Flag | Meaning | Default |
|---|---|---|
| `--home DIR` | Gateway home | see above |
| `--sim` | Register the virtual lab | off |
| `--port N` | Port; `0` picks a free one (the runtime file records it) | `7400` |
| `--listen ADDR` | Address to listen on. Anything but loopback also needs `--allow-remote` | `127.0.0.1` |
| `--allow-remote` | Allow a non-loopback listen address. The API is not designed for the open internet | off |
| `--operator PRINCIPAL` | Principal of the operator token | `operator:local` |
| `--seed N` | Seed for simulator noise | `42` |
| `--profile FILE` | Register the **real** device a protocol profile describes (repeatable) | none |
| `--profile-sim FILE` | Register a profile's simulated device instead (repeatable) | none |
| `--adapter-manifest FILE` | Run an out-of-process adapter from its permission manifest (repeatable) | none |
| `--allow-unsigned-adapters` | Development only: launch adapters whose manifests are unsigned | off |
| `--topology FILE` | How registered devices relate ([spec/topology.md](../spec/topology.md)) | none |

## Clients

All other commands are clients of a running gateway.

| Flag / variable | Meaning |
|---|---|
| `--home DIR`, `OPENMHS_HOME` | Which gateway to talk to |
| `--token T`, `OPENMHS_TOKEN` | Act as the principal this token belongs to, instead of the operator |
| `--json` | Print the raw API result |

Exit codes are documented per command in the [CLI reference](cli.md).

## Other variables

| Variable | Used by |
|---|---|
| `OPENMHS_PYTHON` | The adapter-host tests only: which Python interpreter they use. A Python adapter's interpreter is the `runtime.command` in its manifest |
| `OPENMHS_BIN` | The Python tests, examples and smoke test: which `openmhs` binary to start |

## Device profiles, descriptors and manifests

What a device is, and how to reach it, is data, not gateway configuration:

- a **descriptor** ([spec/descriptor.md](../spec/descriptor.md)) is the contract for one device;
- a **profile** ([device profiles](adapters/profiles.md)) describes a Modbus, MQTT, SCPI or HTTP device;
- a **permission manifest** ([spec/permission-manifest.md](../spec/permission-manifest.md)) declares what an out-of-process adapter may do.

## Transport security

MQTT and HTTP adapters verify TLS by default, and plaintext beyond loopback is refused
unless a profile sets `allow_plaintext`. See [transport security](security/transport-security.md).
