"""MADSci's workcell manager with its in-memory backends (no Redis, no MongoDB, no Docker).

    python workcell.py --port 2100 --node liquid_handler=http://127.0.0.1:2101 --node plate_reader=...

MADSci ships InMemoryCacheHandler and InMemoryDocumentStorageHandler; nothing else about the
manager is changed. Its state does not survive a restart of this process.
"""

from __future__ import annotations

import argparse
import os
import tempfile

from madsci.common.db_handlers.cache_handler import InMemoryCacheHandler
from madsci.common.db_handlers.document_storage_handler import InMemoryDocumentStorageHandler
from madsci.common.types.workcell_types import WorkcellManagerSettings
from madsci.workcell_manager.workcell_server import WorkcellManager


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--node", action="append", default=[], help="name=url")
    a = ap.parse_args()
    # Settings also come from .env / workcell.env in the working directory: run in an empty one.
    os.chdir(tempfile.mkdtemp(prefix="madsci-wc-"))
    settings = WorkcellManagerSettings(
        server_url=f"http://127.0.0.1:{a.port}",
        nodes=dict(n.split("=", 1) for n in a.node),
        scheduler_update_interval=0.1,
        node_update_interval=0.2,
    )
    manager = WorkcellManager(settings=settings, cache_handler=InMemoryCacheHandler(),
                              document_handler=InMemoryDocumentStorageHandler())
    manager.run_server(host="127.0.0.1", port=a.port)


if __name__ == "__main__":
    main()
