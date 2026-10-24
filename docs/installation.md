# Installation

OpenMHS ships two programs, `openmhs` (the command line, which can also run the gateway)
and `openmhsd` (the gateway only). Both are one self-contained binary with the virtual
lab built in; nothing else needs to be installed to try it.

> **No release has been published yet.** Until one is, build from source. The sections
> for release archives and PyPI describe what the first release will provide.

## Platforms

| Platform | Status |
|---|---|
| Windows 11 x86_64 | Built and tested locally and in CI |
| Linux x86_64 (Ubuntu) | Built and tested in CI |
| macOS arm64 | Configured in CI and in the release build; not yet confirmed |
| macOS x86_64 | In the release build; untested |

## From source

You need a Rust toolchain, 1.89 or newer ([rustup](https://rustup.rs)).

```sh
git clone https://github.com/KNambiarDJsc/Open-MHS
cd Open-MHS
cargo install --locked --path crates/openmhs-cli    # installs openmhs and openmhsd
openmhs --version
```

To work on OpenMHS rather than use it, build in place and run from `target/`:

```sh
cargo build -p openmhs-cli
./target/debug/openmhs --version         # Windows: .\target\debug\openmhs.exe
```

## From a release archive

Each release attaches one archive per platform, each next to a `.sha256` file, and a
`sha256.sum` for the whole release:

| Platform | Archive |
|---|---|
| Windows x86_64 | `openmhs-cli-x86_64-pc-windows-msvc.zip` |
| Linux x86_64 | `openmhs-cli-x86_64-unknown-linux-gnu.tar.gz` |
| macOS arm64 | `openmhs-cli-aarch64-apple-darwin.tar.gz` |
| macOS x86_64 | `openmhs-cli-x86_64-apple-darwin.tar.gz` |

Verify the checksum, extract into a new directory, and put the two binaries on your
`PATH`. (The Windows archive is flat; the tarballs may unpack into a directory named after
the archive, which the `find` below locates.)

```sh
# Linux / macOS
sha256sum -c openmhs-cli-x86_64-unknown-linux-gnu.tar.gz.sha256    # macOS: shasum -a 256 -c
mkdir openmhs && tar xzf openmhs-cli-x86_64-unknown-linux-gnu.tar.gz -C openmhs
BIN="$(dirname "$(find openmhs -name openmhs -type f | head -n 1)")"    # the unpacked directory
"$BIN/openmhs" --version
```

```powershell
# Windows
Get-FileHash .\openmhs-cli-x86_64-pc-windows-msvc.zip -Algorithm SHA256   # compare with the .sha256 file
Expand-Archive .\openmhs-cli-x86_64-pc-windows-msvc.zip -DestinationPath openmhs
.\openmhs\openmhs.exe --version
```

Release builds also carry a signed build-provenance attestation. Check one with the
GitHub CLI: `gh attestation verify <archive> --repo KNambiarDJsc/Open-MHS`.

The archives hold only the binaries, the licence, the README and the changelog. The
simulated devices are compiled in, so `openmhs demo` and `openmhs serve --sim` work from
anywhere. Example files such as the [workcell plan](../examples/workcell/README.md) and
the protocol profiles live in the repository, not in the archive.

## The Python client

The Python client is a separate, dependency-free package for talking to a running
gateway. It is not on PyPI yet; install it from a checkout:

```sh
pip install ./python
python -c "import openmhs; print(openmhs.__version__)"
```

It needs Python 3.10 or newer, and a gateway (`openmhs serve`) to talk to.
See the [Python SDK](../python/README.md).

## Optional

- **MCP hosts** (Claude Code, Claude Desktop, …) launch `openmhs mcp`; see
  [Connecting an AI agent](mcp.md). No API key is needed for the scripted example.
- **Python or OPC UA/ROS 2 adapters** need Python, `asyncua`, or the pinned ROS 2
  container; see [OPC UA](adapters/opcua.md) and [ROS 2](adapters/ros2.md).

## Uninstalling

`cargo uninstall openmhs-cli` for a source install; otherwise delete the binaries. The
gateway keeps its state in its [home directory](configuration.md#the-gateway-home); delete
that to remove the journal, policy and tokens.
