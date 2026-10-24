"""Shared code for the adapter registry and the compatibility matrix.

Everything here only reads JSON files. Nothing in a registry entry, a profile or a
conformance report is ever executed, imported or fetched: third-party entries are data.

Requires: pip install jsonschema
"""
import hashlib
import json
import os
import pathlib
import re

from jsonschema import Draft202012Validator, FormatChecker
from referencing import Registry, Resource

# The schemas always come from this checkout. The data being checked (registry entries,
# reports, and the paths they point at) can be another tree, which is how the tests feed
# the checker malformed entries.
SCHEMA_ROOT = pathlib.Path(__file__).resolve().parent.parent
ROOT = pathlib.Path(os.environ.get("OPENMHS_DATA_ROOT", SCHEMA_ROOT)).resolve()
REGISTRY_DIR = ROOT / "registry" / "adapters"
REPORT_DIR = ROOT / "conformance" / "reports"

LEVELS = ["L0", "L1", "L2", "L3"]


def load(path):
    return json.loads(pathlib.Path(path).read_text(encoding="utf-8"))


def _validators():
    schemas = {p.stem.removesuffix(".schema"): load(p) for p in (SCHEMA_ROOT / "spec" / "schemas").glob("*.schema.json")}
    entry = load(SCHEMA_ROOT / "registry" / "schema" / "adapter-entry.schema.json")
    everything = list(schemas.values()) + [entry]
    registry = Registry().with_resources((s["$id"], Resource.from_contents(s)) for s in everything)

    def make(schema):
        return Draft202012Validator(schema, registry=registry, format_checker=FormatChecker())

    return make(entry), make(schemas["conformance-report"])


ENTRY_VALIDATOR, REPORT_VALIDATOR = _validators()


def sha256_file(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


def _is_https(s):
    return s.startswith("https://")


def _repo_path_problem(value, what):
    """Why `value` is not an acceptable path inside this repository, or None."""
    if _is_https(value):
        return None
    if ".." in pathlib.PurePosixPath(value).parts or value.startswith("/") or "\\" in value:
        return f"{what} '{value}' must be a relative path inside the repository"
    if not (ROOT / value).exists():
        return f"{what} '{value}' does not exist"
    return None


def evidence_class(entry, report):
    """How strongly the evidence supports the entry. Derived from the report, never claimed.

    - unverified: no report on file.
    - physically validated: a passing report whose readings did not come from a simulator.
    - simulated: the adapter was tested against the project's in-process virtual lab or an
      adapter's own in-process simulator; no wire protocol was exercised.
    - integration-tested: the adapter spoke its real protocol to a simulated peer.
    """
    if report is None:
        return "unverified"
    sources = set(report["subject"]["sources"])
    complete = report["summary"]["fail"] == 0 and report["summary"]["untested"] == 0
    if "simulator" not in sources and "device" in sources and report["passed"] and complete:
        return "physically validated"
    if entry.get("peer") == "in-process-simulator":
        return "simulated"
    return "integration-tested"


def result_of(report):
    """`pass` only when nothing failed and nothing mandatory was skipped."""
    if report is None:
        return "not verified"
    s = report["summary"]
    if s["fail"]:
        return "fail"
    if s["untested"]:
        return "incomplete"
    return "pass" if report["passed"] else "fail"


def validate_report(report, where):
    problems = [f"{where}: {e.message}" for e in REPORT_VALIDATOR.iter_errors(report)]
    if problems:
        return problems
    counted = {"pass": 0, "fail": 0, "untested": 0, "not_applicable": 0}
    for c in report["checks"]:
        counted[c["status"]] += 1
    if counted != report["summary"]:
        problems.append(f"{where}: summary {report['summary']} does not match its checks {counted}")
    return problems


def load_entries():
    """(entries, problems): every registry entry with its report resolved, and what is wrong."""
    entries, problems = [], []
    for path in sorted(REGISTRY_DIR.glob("*.json")):
        where = path.relative_to(ROOT).as_posix()
        try:
            entry = load(path)
        except (OSError, ValueError) as e:
            problems.append(f"{where}: not readable JSON: {e}")
            continue
        errors = [f"{where}: {e.message}" for e in ENTRY_VALIDATOR.iter_errors(entry)]
        if errors:
            problems += errors
            continue
        if path.stem != entry["id"]:
            problems.append(f"{where}: file name must be {entry['id']}.json")
        for key, what in (("source", "source"),):
            p = _repo_path_problem(entry[key], what)
            if p:
                problems.append(f"{where}: {p}")
        for prof in entry["profiles"]:
            p = _repo_path_problem(prof, "profile")
            if p:
                problems.append(f"{where}: {p}")

        report = None
        ref = entry["report"]
        category = entry["category"]
        if ref is None:
            if category in ("official", "community"):
                problems.append(f"{where}: a {category} adapter needs a conformance report")
        else:
            if category == "unverified":
                problems.append(f"{where}: an entry with a report is not 'unverified'; use experimental, community or official")
            rpath = ROOT / ref["path"]
            if ".." in pathlib.PurePosixPath(ref["path"]).parts or not rpath.is_file():
                problems.append(f"{where}: report '{ref['path']}' does not exist")
            else:
                if sha256_file(rpath) != ref["sha256"]:
                    problems.append(f"{where}: report '{ref['path']}' does not match its recorded sha256")
                try:
                    report = load(rpath)
                except ValueError as e:
                    problems.append(f"{where}: report is not JSON: {e}")
                else:
                    rp = validate_report(report, ref["path"])
                    problems += rp
                    if rp:
                        report = None
            if "peer" not in entry:
                problems.append(f"{where}: an entry with a report must say what it talked to (peer)")
            if report is not None:
                sources = set(report["subject"]["sources"])
                if entry.get("peer") == "device" and ("simulator" in sources or "device" not in sources):
                    problems.append(f"{where}: peer is 'device' but the report's sources are {sorted(sources)}")
                if entry.get("peer") in ("in-process-simulator", "protocol-simulator") and "simulator" not in sources:
                    problems.append(f"{where}: peer is a simulator but the report's sources are {sorted(sources)}")
                if report["subject"]["adapter"] != entry["id"]:
                    problems.append(f"{where}: report is for adapter '{report['subject']['adapter']}', not '{entry['id']}'")
                if report["subject"].get("adapter_version") != entry["version"]:
                    problems.append(f"{where}: report is for version {report['subject'].get('adapter_version')}, entry says {entry['version']}")
                if report.get("declared") != entry["conformance_profile"]:
                    problems.append(f"{where}: report declares {report.get('declared')}, entry says {entry['conformance_profile']}")
                if report["started_at"][:10] != ref["verified_at"]:
                    problems.append(f"{where}: verified_at {ref['verified_at']} is not the report's date {report['started_at'][:10]}")
                if category in ("official", "community"):
                    s = report["summary"]
                    if not report["passed"] or s["fail"] or s["untested"]:
                        problems.append(
                            f"{where}: a {category} entry needs a passing report with nothing failed or untested "
                            f"(passed={report['passed']}, fail={s['fail']}, untested={s['untested']})"
                        )
                if category == "community" and not _is_https(entry["source"]):
                    problems.append(f"{where}: a community adapter's source must be an https URL")
        entries.append({"entry": entry, "report": report, "path": where})
    return entries, problems


def all_reports():
    """Every report file on file, valid or not, oldest first per device. Old reports are kept."""
    out, problems = [], []
    for path in sorted(REPORT_DIR.glob("*/*.json")):
        where = path.relative_to(ROOT).as_posix()
        try:
            r = load(path)
        except ValueError as e:
            problems.append(f"{where}: not JSON: {e}")
            continue
        p = validate_report(r, where)
        problems += p
        if not p:
            out.append((where, r))
    return out, problems


def platform_of(report):
    env = report.get("environment")
    if not env:
        return "unknown"
    return f"{env['os']} {env['arch']}"


def anchor(text):
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
