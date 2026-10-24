"""Validate the adapter registry (registry/README.md).

- every registry/adapters/<id>.json matches registry/schema/adapter-entry.schema.json,
- its report exists under conformance/reports/, matches the recorded SHA-256, is a valid
  conformance report for the same adapter, version and declared level, and, for official
  and community entries, passed with nothing failed or skipped,
- every report on file is valid.

Reads JSON only. It never runs, imports or downloads an adapter.

Usage: python tools/check_registry.py   (requires: pip install jsonschema)
"""
import sys

import registry_lib as lib


def main():
    entries, problems = lib.load_entries()
    reports, report_problems = lib.all_reports()
    problems += report_problems
    if problems:
        print("\n".join(problems))
        return 1
    print(f"ok: {len(entries)} registry entries, {len(reports)} conformance reports")
    return 0


if __name__ == "__main__":
    sys.exit(main())
