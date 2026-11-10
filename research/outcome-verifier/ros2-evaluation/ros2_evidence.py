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

`"time": "clock"` is for unstamped messages in a simulation bag (an action's status topic, for one). The recorder's
receipt time is placed on the simulation clock midway between the two `/clock` messages the same recorder received
just before and just after it. The record says so (`<domain>/recorder`) and carries an uncertainty: half the
simulation time between those two `/clock` messages, plus `clock.receipt_allowance_ms` from the mapping, which states
how late the recorder may receive a message after it was sent. With no `/clock` message on both sides, there is no
source time.

Messages that hold a list (`tf2_msgs/TFMessage`, `action_msgs/GoalStatusArray`) are read element by element with
`"each": "<list field>"`, kept only where `"where": {field: value}` holds, and an element's own header is used if it
has one. `"enum"` names integer values; `"ref_fields"` copies element fields into `refs` (a UUID byte array becomes
its canonical text); `"changes_only": true` keeps an element only when its value differs from the last kept value
with the same refs, so a status array republished unchanged does not repeat a transition.
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


# rosbag2 (Jazzy, 2026-11-10) recorded the hidden action status topic with an empty schema: the type name, no
# definition. These standard definitions are used only then, and every use is listed in the conversion's metadata.
SUPPLIED_DEFINITIONS = {
    "action_msgs/msg/GoalStatusArray": """action_msgs/GoalStatus[] status_list
================================================================================
MSG: action_msgs/GoalStatus
action_msgs/GoalInfo goal_info
int8 status
================================================================================
MSG: action_msgs/GoalInfo
unique_identifier_msgs/UUID goal_id
builtin_interfaces/Time stamp
================================================================================
MSG: unique_identifier_msgs/UUID
uint8[16] uuid
================================================================================
MSG: builtin_interfaces/Time
int32 sec
uint32 nanosec""",
}


class _Decoders(DecoderFactory):
    """The ROS 2 MCAP decoder, plus SUPPLIED_DEFINITIONS for a schema the bag recorded empty."""

    supplied: set[str] = set()

    def decoder_for(self, message_encoding: str, schema: Any) -> Any:
        if schema is not None and not schema.data and message_encoding == "cdr":
            if schema.name not in SUPPLIED_DEFINITIONS:
                raise SystemExit(f"the bag has no message definition for {schema.name}, and the bridge supplies none")
            from mcap_ros2._dynamic import generate_dynamic

            _Decoders.supplied.add(schema.name)
            return generate_dynamic(schema.name, SUPPLIED_DEFINITIONS[schema.name])[schema.name]
        return super().decoder_for(message_encoding, schema)


def messages(paths: list[Path], topics: set[str] | None) -> Iterator[tuple[str, str, int, Any]]:
    """(topic, type, log_time_ns, decoded message), in log-time order within each file."""
    for p in paths:
        with open(p, "rb") as f:
            reader = make_reader(f, decoder_factories=[_Decoders()])
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
            if schema is not None and not schema.data:
                out["topics"][ch.topic]["definition"] = "missing from the bag"
    return out


def field(msg: Any, path: str) -> Any:
    v = msg
    for part in path.split("."):
        v = v[int(part)] if part.isdigit() else getattr(v, part)
    return v


def stamp_ns(header: Any) -> int | None:
    if header is None:
        return None
    s = header.stamp
    ns = int(s.sec) * 1_000_000_000 + int(s.nanosec)
    return ns or None


def ref_text(raw: Any) -> str | None:
    """A ref value as text: a 16-byte UUID array in canonical form, a string or integer as itself."""
    if isinstance(raw, str):
        return raw
    if isinstance(raw, bool):
        return None
    if isinstance(raw, int):
        return str(raw)
    try:
        b = bytes(raw)
    except (TypeError, ValueError):
        return None
    if len(b) != 16:
        return None
    h = b.hex()
    return f"{h[:8]}-{h[8:12]}-{h[12:16]}-{h[16:20]}-{h[20:]}"


class ClockMap:
    """Receipt time to simulation time, from the `/clock` messages the same recorder received."""

    def __init__(self, paths: list[Path]):
        import bisect

        self._bisect = bisect
        pairs = sorted((log_ns, int(msg.clock.sec) * 1_000_000_000 + int(msg.clock.nanosec))
                       for _, _, log_ns, msg in messages(paths, {"/clock"}))
        self.log = [p[0] for p in pairs]
        self.sim = [p[1] for p in pairs]

    def at(self, log_ns: int) -> tuple[int, float] | None:
        """(simulation ns, half-gap ms), or None without a /clock message on both sides."""
        i = self._bisect.bisect_right(self.log, log_ns)
        if i == 0 or i >= len(self.log):
            return None
        before, after = self.sim[i - 1], self.sim[i]
        if after < before:
            return None  # the simulation clock went back (a reset): no honest placement
        return (before + after) // 2, (after - before) / 2 / 1e6


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


def _record(m: dict[str, Any], topic: str, n: int, idx: int | None, log_ns: int, msg: Any, el: Any, domain: str,
            clock: dict[str, Any], clock_map: ClockMap | None, source: dict[str, Any], provenance: str | None,
            counts: Counter) -> dict[str, Any] | None:
    """One evidence record from one message (or one element of a list in it), or None with the reason counted."""
    sig = m["signal"]
    try:
        for k, want in m.get("where", {}).items():
            if field(el, k) != want:
                return None
        raw = field(el, m["field"])
        refs = dict(m.get("refs", {}))
        for k, path in m.get("ref_fields", {}).items():
            text = ref_text(field(el, path))
            if text is None:
                counts[f"{sig}: ref not readable"] += 1
                return None
            refs[k] = text
    except (AttributeError, IndexError, ValueError):
        counts[f"{sig}: field missing"] += 1
        return None
    if "enum" in m:
        name = m["enum"].get(str(raw))
        if name is None:
            counts[f"{sig}: value not in enum"] += 1
            return None
        raw = name
    value = value_of(raw, m.get("unit"))
    if value is None:
        counts[f"{sig}: not a finite value"] += 1
        return None

    header = getattr(el, "header", None) or getattr(msg, "header", None)
    mode = m.get("time")
    uncertainty = clock.get("uncertainty_ms")
    if mode == "log_time":
        src_ns, clock_domain = log_ns, f"{domain}/recorder"
    elif mode == "clock":
        placed = clock_map.at(log_ns) if clock_map else None
        clock_domain = f"{domain}/recorder"
        if placed is None:
            src_ns, uncertainty = None, None
        else:
            src_ns = placed[0]
            uncertainty = round(placed[1] + float(clock["receipt_allowance_ms"]), 3)
    else:
        src_ns, clock_domain = stamp_ns(header), domain
    if src_ns is None:
        counts[f"{sig}: no source stamp"] += 1
    time: dict[str, Any] = {"ingest": rfc3339(log_ns), "clock_domain": clock_domain}
    if src_ns is not None:
        time["source"] = rfc3339(src_ns)
        if uncertainty is not None:
            time["uncertainty_ms"] = uncertainty
    rec: dict[str, Any] = {
        "id": f"{topic}#{n}{'' if idx is None else f'.{idx}'}:{sig}",
        "signal": sig,
        "value": value,
        "source": {k: v for k, v in {"system": source.get("system", "ros2"), "device": m.get("device"),
                                     "kind": m["kind"], "provenance": provenance}.items() if v is not None},
        "time": time,
        "_src_ns": src_ns,
    }
    frame = m.get("frame") or (getattr(header, "frame_id", "") if header is not None else "")
    if frame:
        rec["frame"] = frame
    if refs:
        rec["refs"] = refs
    return rec


def convert(path: Path, mapping: dict[str, Any]) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    _Decoders.supplied = set()
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
        if m.get("time") not in (None, "log_time", "clock"):
            raise SystemExit(f"mapping for {m['topic']}: time must be log_time, clock, or absent (header.stamp)")
        by_topic[m["topic"]].append(m)

    clock_map = None
    if any(m.get("time") == "clock" for ms in by_topic.values() for m in ms):
        if not domain.startswith("sim:") or "/clock" not in info["topics"]:
            raise SystemExit("time 'clock' needs a simulation bag: a recorded /clock and a 'sim:' clock domain")
        if clock.get("receipt_allowance_ms") is None:
            raise SystemExit("time 'clock' needs clock.receipt_allowance_ms: how late the recorder may receive a message")
        clock_map = ClockMap(paths)

    window = mapping.get("time_range_ns")
    records: list[dict[str, Any]] = []
    counts: Counter = Counter()
    last_kept: dict[str, int] = {}
    last_value: dict[tuple, Any] = {}
    seq: Counter = Counter()
    for topic, mtype, log_ns, msg in messages(paths, set(by_topic)):
        seq[topic] += 1
        if window and not (window[0] <= log_ns <= window[1]):
            continue
        for m in by_topic[topic]:
            sig = m["signal"]
            if m.get("each"):
                try:
                    elements = list(enumerate(field(msg, m["each"])))
                except (AttributeError, IndexError, ValueError):
                    counts[f"{sig}: field missing"] += 1
                    continue
            else:
                elements = [(None, msg)]
            for idx, el in elements:
                rec = _record(m, topic, seq[topic], idx, log_ns, msg, el, domain, clock, clock_map, source, provenance,
                              counts)
                if rec is None:
                    continue
                if m.get("changes_only"):
                    key = (sig, tuple(sorted(rec.get("refs", {}).items())))
                    if key in last_value and last_value[key] == rec["value"]:
                        counts[f"{sig}: unchanged"] += 1
                        continue
                    last_value[key] = rec["value"]
                src_ns = rec.pop("_src_ns")
                if m.get("every_ms") and src_ns is not None:
                    if src_ns - last_kept.get(sig, -(10**30)) < m["every_ms"] * 1_000_000:
                        counts[f"{sig}: decimated"] += 1
                        continue
                    last_kept[sig] = src_ns
                records.append(rec)
                counts[f"{sig}: records"] += 1
    records.sort(key=lambda r: r["id"])
    meta = {
        "bag": [p.name for p in paths],
        "bag_sha256": sha256_of(paths),
        "source": source,
        "clock": clock,
        "topics_used": {t: info["topics"][t] for t in sorted(by_topic)},
        "counts": dict(sorted(counts.items())),
        "definitions_supplied_by_bridge": sorted(_Decoders.supplied),
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
