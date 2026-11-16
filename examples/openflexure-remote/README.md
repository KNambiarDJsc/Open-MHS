# OpenMHS Remote Researcher — simulator-only proof of concept

**Status: proof of concept, not deployable for real remote access.** Demonstrates scoped external-researcher *identity* with the existing OpenMHS gateway and OpenFlexure simulator, without a new business platform.

## What is implemented

The host operator creates a distinct `agent:remote-researcher:<session>` credential, for a maximum of 60 minutes, scoped exclusively to `read` on `ofm.stage` `position.x/y/z`. The existing OpenMHS scope **and** grant checks still apply. A researcher retrieves three position readings from the gateway; OpenMHS audits the reads. The operator can revoke the token.

There is **no remotely accessible transport**, scheduling, browser frontend, image sharing, live experiments, approved movement, remote identity federation, multi-tenant isolation, or physical validation in this PoC. Deliberately, the researcher cannot use `openflexure-imaging`: that tool currently talks directly to OpenFlexure's camera API and is not protected by the gateway. The host continues to operate the simulator.

## Reproduce locally

First start the OpenFlexure simulator and the **existing** [OpenFlexure agent kit](../openflexure-agent/README.md) using its normal local startup instructions, but don't attach the imaging server to the researcher. Use `start.py` **without** `--approval` only in simulator contexts; the technician principal from that example is **not** a researcher principal and must not be shared.

1. On the host, in the OpenMHS checkout:

   ```sh
   python examples/openflexure-remote/remote_researcher.py issue --home ~/.openmhs-openflexure --session college-b-001 --minutes 15
   ```

   Record the printed token ID for revocation and communicate the secret token only via a secure channel. **The token is printed once; don't paste it into tickets, logs or screenshots.** The host must not share its operator token or `mcp-config.json`.

2. In another local shell (as the simulated researcher), use the gateway's loopback URL from its `gateway.json` runtime metadata. **Do not copy its operator token.** Set `OPENMHS_REMOTE_TOKEN` to the researcher token and invoke:

   ```sh
   python examples/openflexure-remote/remote_researcher.py observe --url http://127.0.0.1:PORT
   ```

   The output marks unusable/stale reads rather than treating them as validated measurements. Other devices, write and invoke must be denied. A complete smoke test should assert this negative behavior.

3. On the host:

   ```sh
   python examples/openflexure-remote/remote_researcher.py revoke --home ~/.openmhs-openflexure --token-id TOKEN_ID
   ```

   Subsequent reads with the revoked credential must fail. Read grant IDs remain for the session principal; don't reuse session principal identifiers.

## Next steps before any network deployment

- Add a session state machine with expiration, cancellation and durable binding to resource/lease.
- Add independent lab-side identity provider and remote researcher authentication, TLS/mTLS, audited invitation and revocation; perform threat modeling.
- Build an **outbound-only, authenticated** remote session transport, with quotas and no inbound lab network access. Validate operator override and end-to-end revocation; do not assume the present HTTP gateway can safely listen on a public interface.
- Move camera capture into an OpenMHS-approved capability or audited proxy, not the current direct imaging MCP helper.
- Implement a reviewed per-session, bounded *procedure* with host approval before any physical movement; keep hardware interlocks and local operator stop mechanisms authoritative.
- Test a full remote simulation (two separated hosts, network loss, identity changes, timeout after execution, stale observations) before requesting a supervised read-only real-hardware pilot.

## Limits

This example does not make a microscope securely accessible over the Internet. It demonstrates only the *authorization boundary* using current APIs. It is not a safety system. No physical devices were tested.
