# ADR-0008: Rust for the trusted core, Python for clients and out-of-process adapters

- Status: Accepted
- Date: 2026-10-23

## Context

The gateway is the trusted component. It should be a single portable binary with
predictable resource use. The wider community (lab researchers, robotics engineers)
works mostly in Python, and the best OPC UA and ROS 2 client libraries are in
Python (`asyncua`, `rclpy`).

## Decision

- Rust (stable; 1.99.1 at time of writing, MSVC toolchain on Windows) for the
  gateway, engine, policy, simulator framework, CLI and MCP server.
- Python for the client SDK (M4, pure Python, no compiler needed) and the adapter
  SDK (M7, adapters run out-of-process over a JSON-RPC stdio protocol).
- JSON Schema files in `spec/` are the language-neutral contract between the two.

Rust is not a safety argument by itself. Assurance comes from specified invariants,
fault testing, independent interlocks, correct deployment and external review.

## Consequences

- Two language toolchains in CI.
- Python adapters cannot crash the gateway (process isolation), which is a feature.
