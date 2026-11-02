"""MADSci's data manager with its in-memory backends. The workcell manager stores every
step's result in it as a datapoint and keeps only the datapoint's id in the step.

    python data.py --port 2104
"""

from __future__ import annotations

import argparse
import os
import tempfile

from madsci.common.db_handlers.document_storage_handler import InMemoryDocumentStorageHandler
from madsci.common.db_handlers.object_storage_handler import InMemoryObjectStorageHandler
from madsci.common.types.datapoint_types import DataManagerSettings
from madsci.data_manager.data_server import DataManager


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, required=True)
    a = ap.parse_args()
    os.chdir(tempfile.mkdtemp(prefix="madsci-data-"))
    DataManager(settings=DataManagerSettings(server_url=f"http://127.0.0.1:{a.port}"),
                document_handler=InMemoryDocumentStorageHandler(),
                object_storage_handler=InMemoryObjectStorageHandler()).run_server(host="127.0.0.1", port=a.port)


if __name__ == "__main__":
    main()
