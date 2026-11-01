"""MADSci's location manager with its in-memory backends. The workcell manager needs one to
start any workflow (it lists locations for every step), though this experiment uses none.

    python location.py --port 2103
"""

from __future__ import annotations

import argparse
import os
import tempfile

from madsci.common.db_handlers.cache_handler import InMemoryCacheHandler
from madsci.common.db_handlers.document_storage_handler import InMemoryDocumentStorageHandler
from madsci.common.types.location_types import LocationManagerSettings
from madsci.location_manager.location_server import LocationManager


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, required=True)
    a = ap.parse_args()
    os.chdir(tempfile.mkdtemp(prefix="madsci-loc-"))
    settings = LocationManagerSettings(server_url=f"http://127.0.0.1:{a.port}")
    LocationManager(settings=settings, cache_handler=InMemoryCacheHandler(),
                    document_handler=InMemoryDocumentStorageHandler()).run_server(host="127.0.0.1", port=a.port)


if __name__ == "__main__":
    main()
