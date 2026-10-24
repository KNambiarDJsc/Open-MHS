# README: before and after

What changed in the M10 README rewrite, and why. The old README (M0–M9) was a project
status page; the new one is an entry point for a stranger.

## Before

| Section | Problem |
|---|---|
| Status blurb | Said "M7" while the code was at M9; a reader had to know milestones to read it |
| "What it aims to be" | Abstract, no command to run |
| "What works today" | A 24-row table of internal components, each with a crate name; hard to scan, no demo, mixed implemented and planned work |
| "Try it" | Led with `cargo run`, a developer command, and a long list of scenarios; no sense of what a user would see |
| "Tests" | Developer content above the fold of the document |
| Known limitations | Accurate but buried near the end, and stale in two places |
| No images, no badges, no community links | Nothing to tell a newcomer whether the project was alive or how to take part |

Stale or wrong statements found in the audit
([M10_RELEASE_AUDIT.md](M10_RELEASE_AUDIT.md)): "M7" status; "no TLS for MQTT or HTTP yet"
(verified TLS shipped in M7); "protocol adapters are L2" (they reach L3 against their
simulators, though their profiles declare L2).

## After

The thirteen sections the M10 brief asked for, in order:

1. **Hero:** name, the tagline *Connect AI agents to programmable hardware.*, two sentences,
   four badges that are true (CI, licence, Rust version, pre-alpha), links to quickstart,
   documentation, compatibility and contributing.
2. **Demo above the fold:** an animated terminal (SVG) of a real run, with alt text and a
   plain-text transcript as the fallback.
3. **Why OpenMHS:** three problems (guessing at units and limits; permission in the prompt;
   ambiguous timeouts) and what OpenMHS does about each.
4. **Quickstart:** about a minute, Unix and PowerShell, real commands whose output was captured
   from a real gateway and re-run from a release binary extracted into an empty directory.
   It says plainly that there is no published release yet and to build from source.
5. **Features,** as short statements with links.
6. **How it works:** one Mermaid diagram, agents to devices and back.
7. **Supported hardware and protocols,** a table **generated** from conformance reports by
   `tools/gen_matrix.py` and checked for staleness in CI, with the rule that simulation is not
   physical validation.
8. **Three examples:** a simulated sensor, a simulated programmable device (and the workcell
   plan), and an MCP agent.
9. **OpenMHS and other approaches:** Anthropic's MHS (no compatibility claim), ROS 2, OPC UA,
   vendor SDKs, per-instrument MCP servers.
10. **Safety and known limitations,** moved up and made current.
11. **Documentation** links.
12. **Contributing:** "Want to connect a new device? Build an adapter."
13. **Community, acknowledgements, licence.**

Removed from the README: the 24-row internal status table (the old README is in git
history; milestones are in the [roadmap](../roadmap.md) and
[docs/IMPLEMENTATION_STATUS.md](../IMPLEMENTATION_STATUS.md)), the
developer test commands (now [CONTRIBUTING.md](../../CONTRIBUTING.md)), and the long list of
profile-adapter scenarios (now [docs/simulators.md](../simulators.md)).

## Claims, and what backs them

| README says | Evidence |
|---|---|
| The quickstart commands work | Run against a debug build, then against the dist-built Windows release binary extracted into an empty directory, in PowerShell |
| `openmhs demo` takes about 10 seconds | Measured: 13 s in a debug build while other builds were running; faster in release |
| Every row in the matrix | A committed conformance report, hash-pinned by the registry entry |
| No row is physically validated | True: every report's `sources` is `["simulator"]`, and the generator would label a passing non-simulator report as physically validated |
| No physical hardware has been tested | True. This is stated in the hero notice, the matrix, the limitations and the release notes template |
| Independent of Anthropic | ADR-0007; stated in the hero notice and the comparison table |
