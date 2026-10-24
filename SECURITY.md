# Security policy

OpenMHS sits between AI agents and programmable hardware, so security reports matter.
It is pre-alpha software and has **not** been security audited. Its threat model and
known gaps are in [docs/security/](docs/security/).

## Reporting a vulnerability

Please **do not open a public issue** for a vulnerability.

Report it privately through GitHub: on the repository, open the **Security** tab and
choose **Report a vulnerability** (a private security advisory). Include what you found,
how to reproduce it, the version or commit, and what an attacker gains.

If that option is not available, open a public issue that says only that you have a
security report and want a private channel. Do not put details in it.

What to expect from a single-maintainer project: an acknowledgement within about 7 days,
an assessment and plan as soon as one is known, and credit in the advisory if you want
it. There is no bug bounty.

## Supported versions

Only the latest commit on `main` and the most recent release, once there is one, are
supported. Before 1.0, fixes are not backported.

## In scope

- Bypassing authorization, scoping, approval, range, unit or rate checks
- Forging or hiding audit records; breaking the hash chain
- An adapter escaping its declared permissions through the adapter host
- Unauthenticated or cross-principal access to the local API or MCP server
- Credential leaks in logs, journals or error messages
- Prompt injection that makes the MCP server treat device-supplied text as instructions

## Out of scope, or known

- Anything that needs local code execution as the same OS user that runs the gateway:
  the gateway home (including the operator token) is a single-user trust boundary.
- The adapter host has no OS sandbox; permission manifests are declared and checked, not
  enforced by the operating system.
- Physical safety. OpenMHS is not a safety system and makes no safety-certification
  claim. Emergency stops belong to the hardware.
- Denial of service against a gateway you exposed beyond loopback on purpose.
