"""Record OctoPrint 1.x's real answers for the offline tests.

    OPENMHS_OCTOPRINT=<path to octoprint> python capture_fixtures.py

Starts a throwaway OctoPrint with the Virtual Printer, saves the response bodies the
adapter relies on to ``fixtures/``, and records the OctoPrint version. The API key never
appears in a response; the files are verbatim.
"""

import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import octoprint_harness as h  # noqa: E402

QUERY = "/api/printer?history=true&limit=1&exclude=sd,state"


def save(name: str, status: int, body) -> None:
    (HERE / "fixtures" / f"{name}.json").write_text(
        json.dumps({"status": status, "body": body}, indent=2) + "\n", encoding="utf-8"
    )


def main() -> int:
    exe = h.octoprint_executable()
    if exe is None:
        print("set OPENMHS_OCTOPRINT to the octoprint executable", file=sys.stderr)
        return 2
    (HERE / "fixtures").mkdir(exist_ok=True)
    with h.OctoPrintServer(exe) as octo:
        status, version = octo.request("GET", "/api/version")
        (HERE / "fixtures" / "VERSION").write_text(version["text"] + "\n", encoding="utf-8")
        save("connection-operational", *octo.request("GET", "/api/connection"))
        save("printer-operational", *octo.request("GET", QUERY))
        octo.disconnect_printer()
        save("connection-closed", *octo.request("GET", "/api/connection"))
        save("printer-offline-409", *octo.request("GET", QUERY))
        status, _ = octo.request("POST", "/api/connection", {"command": "connect", "port": "VIRTUAL", "baudrate": 115200})
        for _ in range(100):  # best effort: catch the moment before the first temperature poll
            status, body = octo.request("GET", QUERY)
            if status == 200 and body["temperature"].get("tool0", {}).get("actual") is None:
                save("printer-just-connected", status, body)
                print("caught a null temperature")
                break
        octo.wait_until("Operational")
        print("saved", version["text"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
