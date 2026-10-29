"""Import a W3C Thing Description into an OpenMHS adapter manifest.

    python import_td.py --td http://127.0.0.1:5000/api/v3/thing_descriptions/ --thing stage \\
        --out ./stage --device-id ofm.stage --source simulator [--overrides reviewed.json]

``--td`` is a URL or a file holding either one Thing Description or a mapping of names to
Thing Descriptions (what LabThings serves at ``/thing_descriptions/``; pick one with
``--thing``). It writes into ``--out``:

* ``manifest.json``   the adapter manifest to pass to ``openmhs serve --adapter-manifest``
  (unsigned: use ``--allow-unsigned-adapters`` for development);
* ``review.json``     what was mapped, what needs review, what is unsupported, and why;
* ``overrides.template.json``  a skeleton for the reviewer, one entry per action that was not
  exposed. Copy it, fill it in, and run the import again with ``--overrides``.

Importing grants nothing: OpenMHS stays deny-by-default.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import urllib.request
from pathlib import Path
from urllib.parse import urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parent))
import td_import  # noqa: E402

IMPORTER_VERSION = "wot-import 0.1"


def load(source: str) -> object:
    if source.startswith(("http://", "https://")):
        with urllib.request.urlopen(source, timeout=15) as r:  # noqa: S310
            return json.loads(r.read())
    return json.loads(Path(source).read_text(encoding="utf-8"))


def build_manifest(mapping: td_import.Mapping, base_url: str, out: Path, *, source: str,
                   timeout_ms: int, stamp: str = "received") -> dict:
    caps = mapping.descriptor["capabilities"]
    parts = urlsplit(base_url)
    origin = f"{parts.scheme}://{parts.netloc}"
    return {
        "openmhs_manifest": "0.1",
        "adapter": {"id": mapping.descriptor["adapter"]["id"], "version": "0.1.0"},
        "runtime": {
            "command": [sys.executable, str(Path(__file__).resolve().parent / "wot_adapter.py")],
            "protocol": ["0.1"],
        },
        "capabilities": {"allow": [{"name": c["name"], "operations": c["operations"]} for c in caps]},
        "endpoints": {"network": [origin]},
        **({"credentials": ["WOT_USER", "WOT_PASSWORD"] if mapping.auth["scheme"] == "basic" else ["WOT_TOKEN"]}
           if mapping.auth else {}),
        "filesystem": {"write": [str(out.resolve())]},
        "config": {
            "descriptor": mapping.descriptor,
            "bindings": mapping.bindings,
            "timeout_ms": timeout_ms,
            "poll_ms": 100,
            "source": source,
            "stamp": stamp,
            "auth": mapping.auth,
            "ledger": str((out / "invocations.jsonl").resolve()),
        },
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--td", required=True, help="URL or file of a Thing Description (or a mapping of them)")
    ap.add_argument("--thing", help="the Thing's name when --td holds several")
    ap.add_argument("--base-url", help="where the Thing is served, if its description has no usable base")
    ap.add_argument("--device-id", help="the OpenMHS device id (default: wot.<title>)")
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--overrides", type=Path, help="the reviewed overrides file")
    ap.add_argument("--source", default="device", choices=["device", "simulator"],
                    help="what readings through this adapter come from")
    ap.add_argument("--timeout-ms", type=int, default=2000)
    ap.add_argument("--stamp", default="received", choices=["received", "date_header"],
                    help="time to give a reading: when received, or the server's HTTP Date")
    ap.add_argument("--probe", action="store_true",
                    help="read (GET) map-valued properties once to learn their keys; reads only")
    args = ap.parse_args()

    doc = load(args.td)
    if isinstance(doc, dict) and "properties" not in doc and "actions" not in doc and "@context" not in doc:
        if not args.thing or args.thing not in doc:
            print(f"error: --td holds several Things ({', '.join(doc)}); pick one with --thing", file=sys.stderr)
            return 2
        doc = doc[args.thing]
    overrides = json.loads(args.overrides.read_text(encoding="utf-8")) if args.overrides else None
    base = args.base_url or doc.get("base") or (args.td.rsplit("/", 2)[0] + "/" if args.td.startswith("http") else "")
    samples = {}
    if args.probe:
        from urllib.parse import urljoin
        for pname, prop in (doc.get("properties") or {}).items():
            if prop.get("type") == "object" and isinstance(prop.get("additionalProperties"), dict):
                form = td_import._form(prop.get("forms"), "readproperty")
                if form:
                    try:
                        samples[pname] = load(urljoin(base, form["href"]))
                    except Exception as e:  # a probe that fails just leaves the property unmapped
                        print(f"note: could not probe {pname}: {e}", file=sys.stderr)
    mapping = td_import.map_thing(doc, base_url=base, device_id=args.device_id, overrides=overrides,
                                  samples=samples)

    # Where this contract came from, so it is part of what the gateway hashes at registration:
    # the exact description imported (canonical sha256), where it was fetched, the reviewed
    # overrides, and the importer's version. A changed Thing Description is then a changed hash.
    canon = lambda v: json.dumps(v, sort_keys=True, separators=(",", ":"), ensure_ascii=False)  # noqa: E731
    mapping.descriptor.setdefault("extensions", {})["x-wot-source"] = {
        "td_sha256": hashlib.sha256(canon(doc).encode("utf-8")).hexdigest(),
        "td_location": args.td if args.td.startswith("http") else Path(args.td).name,
        "importer": IMPORTER_VERSION,
    }
    if overrides:
        mapping.descriptor["extensions"]["x-wot-source"]["overrides_sha256"] = hashlib.sha256(
            canon(overrides).encode("utf-8")).hexdigest()

    args.out.mkdir(parents=True, exist_ok=True)
    if not mapping.connectable:
        review = {"device": mapping.descriptor["device"]["id"], "counts": mapping.counts(),
                  "findings": [f.__dict__ for f in mapping.findings]}
        (args.out / "review.json").write_text(json.dumps(review, indent=2) + "\n", encoding="utf-8")
        why = next(f.detail for f in mapping.findings if f.category == "security_scheme_unsupported")
        print(f"error: {why}", file=sys.stderr)
        return 3
    manifest = build_manifest(mapping, base, args.out, source=args.source, timeout_ms=args.timeout_ms,
                              stamp=args.stamp)
    (args.out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    review = {
        "device": mapping.descriptor["device"]["id"],
        "counts": mapping.counts(),
        "categories": mapping.by_category(),
        "findings": [f.__dict__ for f in mapping.findings],
    }
    (args.out / "review.json").write_text(json.dumps(review, indent=2) + "\n", encoding="utf-8")
    (args.out / "overrides.template.json").write_text(
        json.dumps(td_import.review_template(doc, mapping), indent=2) + "\n", encoding="utf-8")

    c = mapping.counts()
    caps = mapping.descriptor["capabilities"]
    print(f"{mapping.descriptor['device']['id']}: {len(caps)} capabilities "
          f"({sum(1 for x in caps if x['kind'] == 'property')} readable properties, "
          f"{sum(1 for x in caps if x['kind'] == 'action')} actions)")
    for kind, n in c.items():
        print(f"  {kind}: mapped {n['mapped']}, needs review {n['review']}, unsupported {n['unsupported']}")
    print(f"wrote {args.out / 'manifest.json'}, review.json and overrides.template.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())
