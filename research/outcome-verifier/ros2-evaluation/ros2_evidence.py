"""Turn ROS 2 messages from a recorded bag (rosbag2, MCAP storage) into RFC 0004 evidence records.

Read-only: it opens a file. It never subscribes, publishes, calls a service or sends a goal.

    python ros2_evidence.py inspect BAG
    python ros2_evidence.py convert BAG --mapping MAPPING.json --out EVIDENCE.jsonl

BAG is a `.mcap` file or a rosbag2 directory of them. MAPPING says which topics become which signals, and
everything a message cannot say about itself:

- `source.provenance`: `device`, `simulator`, or absent for unknown. Never guessed.
- `clock.domain` and `clock.uncertainty_ms`: which clock stamped the messages and how far it may be from true.
  A bag that carries `/clock` is simulation time, and is refused unless the domain says so (`sim:...`).
- per mapping, `kind` (`device_state`, `independent_sensor`, `controller`, ...): whether a sensor is
  independent of what it checks is the mapping author's claim, stated in the file where it can be challenged.

Each record keeps the message's own `header.stamp` as its source time. When a message has no header, or its stamp
is zero, it gets no source time, unless the mapping says `"time": "log_time"` for that topic, which uses the
recorder's receipt time and says so in the clock domain (`<domain>/recorder`). The recorder's receipt time is always
kept as `time.ingest`. Frames come from `header.frame_id`. Missing fields are skipped and counted, never filled.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import sys
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any, Iterator

from mcap.reader import make_reader
from mcap_ros2.decoder import DecoderFactory


def bag_files(path: Path) -> list[Path]:
    if path.is_dir():
        files = sorted(path.glob("*.mcap"))
        if not files:
            raise SystemExit(f"{path}: no .mcap files (only MCAP storage is supported; convert .db3 with `ros2 bag convert`)")
        return files
    if path.suffix != ".mcap":
        raise SystemExit(f"{path}: not an .mcap file")
    return [path]


def sha256_of(paths: list[Path]) -> str:
    h = hashlib.sha256()
    for p in paths:
        with open(p, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                h.update(chunk)
    return h.hexdigest()


def messages(paths: list[Path], topics: set[str] | None) -> Iterator[tuple[str, str, int, Any]]:
    """(topic, type, log_time_ns, decoded message), in log-time order within each file."""
    for p in paths:
        with open(p, "rb") as f:
            reader = make_reader(f, decoder_factories=[DecoderFactory()])
            for schema, channel, message, decoded in reader.iter_decoded_messages(topics=topics, log_time_order=True):
                yield channel.topic, schema.name if schema else "?", message.log_time, decoded


def inspect(path: Path) -> dict[str, Any]:
    paths = bag_files(path)
    out: dict[str, Any] = {"files": [str(p) for p in paths], "topics": {}}
    for p in paths:
        with open(p, "rb") as f:
            summary = make_reader(f).get_summary()
        if summary is None:
            continue
        stats = summary.statistics
        out["start_ns"], out["end_ns"] = stats.message_start_time, stats.message_end_time
        for cid, ch in summary.channels.items():
            schema = summary.schemas.get(ch.schema_id)
            out["topics"][ch.topic] = {"type": schema.name if schema else "?",
                                       "messages": stats.channel_message_counts.get(cid, 0)}
    return out


def field(msg: Any, path: str) -> Any:
    v = msg
    for part in path.split("."):
        v = v[int(part)] if part.isdigit() else getattr(v, part)
    return v


def stamp_ns(msg: Any) -> int | None:
    header = getattr(msg, "header", None)
    if header is None:
        return None
    s = header.stamp
    ns = int(s.sec) * 1_000_000_000 + int(s.nanosec)
    return ns or None


def rfc3339(ns: int) -> str:
    import datetime as dt

    seconds, rem = divmod(ns, 1_000_000_000)
    base = dt.datetime.fromtimestamp(seconds, dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S")
    return f"{base}.{rem // 1_000_000:03d}Z"


def value_of(raw: Any, unit: str | None) -> Any:
    if isinstance(raw, bool) or isinstance(raw, str):
        return raw
    if isinstance(raw, (int, float)):
        if isinstance(raw, float) and not math.isfinite(raw):
            return None  # not a measurement; skipped and counted
        return {"value": float(raw), "unit": unit} if unit else raw
    return None


def convert(path: Path, mapping: dict[str, Any]) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    paths = bag_files(path)
    info = inspect(path)
    source = mapping.get("source", {})
    clock = mapping.get("clock", {})
    domain = clock.get("domain")
    if not domain:
        raise SystemExit("mapping: clock.domain is required; the bridge does not assume which clock stamped the data")
    if "/clock" in info["topics"] and not domain.startswith("sim:"):
        raise SystemExit(f"bag carries /clock (simulation time); clock.domain must start with 'sim:', not {domain!r}")
    provenance = source.get("provenance")
    if provenance not in (None, "device", "simulator"):
        raise SystemExit("mapping: source.provenance must be device, simulator, or absent for unknown")

    by_topic: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for m in mapping["mappings"]:
        if m["topic"] not in info["topics"]:
            raise SystemExit(f"mapping names {m['topic']}, which is not in the bag (topics: {sorted(info['topics'])})")
        by_topic[m["topic"]].append(m)

    window = mapping.get("time_range_ns")
    records: list[dict[str, Any]] = []
    counts: Counter = Counter()
    last_kept: dict[str, int] = {}
    seq: Counter = Counter()
    for topic, mtype, log_ns, msg in messages(paths, set(by_topic)):
        seq[topic] += 1
        if window and not (window[0] <= log_ns <= window[1]):
            continue
        for m in by_topic[topic]:
            sig = m["signal"]
            uses_log_time = m.get("time") == "log_time"
            src_ns = log_ns if uses_log_time else stamp_ns(msg)
            if m.get("every_ms") and src_ns is not None:
                if src_ns - last_kept.get(sig, -(10**30)) < m["every_ms"] * 1_000_000:
                    counts[f"{sig}: decimated"] += 1
                    continue
            try:
                raw = field(msg, m["field"])
            except (AttributeError, IndexError, ValueError):
                counts[f"{sig}: field missing"] += 1
                continue
            value = value_of(raw, m.get("unit"))
            if value is None:
                counts[f"{sig}: not a finite value"] += 1
                continue
            if src_ns is None:
                counts[f"{sig}: no source stamp"] += 1
            time: dict[str, Any] = {"ingest": rfc3339(log_ns),
                                    "clock_domain": f"{domain}/recorder" if uses_log_time else domain}
            if src_ns is not None:
                time["source"] = rfc3339(src_ns)
            if clock.get("uncertainty_ms") is not None:
                time["uncertainty_ms"] = clock["uncertainty_ms"]
            rec: dict[str, Any] = {
                "id": f"{topic}#{seq[topic]}:{sig}",
                "signal": sig,
                "value": value,
                "source": {k: v for k, v in {"system": source.get("system", "ros2"), "device": m.get("device"),
                                             "kind": m["kind"], "provenance": provenance}.items() if v is not None},
                "time": time,
                "refs": dict(m.get("refs", {})),
            }
            header = getattr(msg, "header", None)
            frame = m.get("frame") or (getattr(header, "frame_id", "") if header is not None else "")
            if frame:
                rec["frame"] = frame
            if not rec["refs"]:
                del rec["refs"]
            records.append(rec)
            if src_ns is not None:
                last_kept[sig] = src_ns
            counts[f"{sig}: records"] += 1
    records.sort(key=lambda r: r["id"])
    meta = {
        "bag": [p.name for p in paths],
        "bag_sha256": sha256_of(paths),
        "source": source,
        "clock": clock,
        "topics_used": {t: info["topics"][t] for t in sorted(by_topic)},
        "counts": dict(sorted(counts.items())),
    }
    return records, meta


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    i = sub.add_parser("inspect")
    i.add_argument("bag", type=Path)
    c = sub.add_parser("convert")
    c.add_argument("bag", type=Path)
    c.add_argument("--mapping", type=Path, required=True)
    c.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()
    if args.cmd == "inspect":
        print(json.dumps(inspect(args.bag), indent=2))
        return 0
    mapping = json.loads(args.mapping.read_text(encoding="utf-8"))
    records, meta = convert(args.bag, mapping)
    args.out.write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in records), encoding="utf-8", newline="\n")
    args.out.with_suffix(".meta.json").write_text(json.dumps(meta, indent=2, sort_keys=True) + "\n", encoding="utf-8",
                                                  newline="\n")
    print(json.dumps(meta["counts"], indent=1), file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
