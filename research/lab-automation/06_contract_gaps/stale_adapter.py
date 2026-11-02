"""Run a lab adapter with one test hook: its readings can be aged on request.

    python stale_adapter.py <path to the real adapter>

While the file named by ``OPENMHS_STALE_FLAG`` exists, every reading carries a timestamp an hour
old, as a device behind a cached or failed feed would report. Nothing else changes, so the
scenario tests the gateway's handling of an old reading from this ecosystem's real adapter.
It also writes the process id next to the flag, so a scenario can kill the adapter alone.
"""

from __future__ import annotations

import importlib.util
import os
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

from openmhs.adapter import Adapter, run

FLAG = Path(os.environ["OPENMHS_STALE_FLAG"])


def main() -> None:
    target = Path(sys.argv[1]).resolve()
    sys.path.insert(0, str(target.parent))
    spec = importlib.util.spec_from_file_location(target.stem, target)
    module = importlib.util.module_from_spec(spec)
    sys.modules[target.stem] = module
    spec.loader.exec_module(module)
    real = next(v for v in vars(module).values()
                if isinstance(v, type) and issubclass(v, Adapter) and v is not Adapter)

    class Aged(real):
        async def read(self, capability):
            value = await super().read(capability)
            if FLAG.exists():
                old = datetime.now(timezone.utc) - timedelta(hours=1)
                value = {**value, "timestamp": old.isoformat().replace("+00:00", "Z")}
            return value

    FLAG.with_suffix(".pid").write_text(str(os.getpid()))
    run(Aged())


if __name__ == "__main__":
    main()
