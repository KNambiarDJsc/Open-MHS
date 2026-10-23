"""Finding a local gateway: its home directory and runtime file (spec/api.md)."""

from __future__ import annotations

import json
import os
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

RUNTIME_FILE = "gateway.json"
HOME_ENV = "OPENMHS_HOME"


def default_home() -> Path:
    """``$OPENMHS_HOME``, else the per-user local data directory + ``openmhs``.

    Matches the gateway's choice on each platform (the Rust ``dirs::data_local_dir``).
    """
    env = os.environ.get(HOME_ENV)
    if env:
        return Path(env)
    if sys.platform == "win32":
        base = os.environ.get("LOCALAPPDATA")
    elif sys.platform == "darwin":
        base = str(Path.home() / "Library" / "Application Support")
    else:
        base = os.environ.get("XDG_DATA_HOME") or str(Path.home() / ".local" / "share")
    return Path(base or tempfile.gettempdir()) / "openmhs"


@dataclass(frozen=True)
class RuntimeInfo:
    """What the gateway writes to ``gateway.json`` on start."""

    url: str
    pid: int
    principal: str
    token: str
    started_at: str

    def __repr__(self) -> str:  # never print the operator token
        return f"RuntimeInfo(url={self.url!r}, pid={self.pid}, principal={self.principal!r})"

    @classmethod
    def load(cls, home: Path | str) -> RuntimeInfo:
        data = json.loads((Path(home) / RUNTIME_FILE).read_text(encoding="utf-8"))
        return cls(
            url=data["url"],
            pid=data["pid"],
            principal=data["principal"],
            token=data["token"],
            started_at=data["started_at"],
        )
