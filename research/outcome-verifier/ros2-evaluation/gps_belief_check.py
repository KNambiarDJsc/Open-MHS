"""Is the consumer GPS's error real, or an artefact of how the experiment calibrated it?

    python gps_belief_check.py --evidence EVIDENCE.jsonl --tum GT_TUM.txt

Fits the best constant offset over the whole run (in hindsight, which no robot could do) at several time lags, and
prints how the raw offset changes over the run. If a constant offset or a lag explained the error, the experiment's
start-of-run calibration would be the problem, not the receiver.
"""

from __future__ import annotations

import argparse
import math
import statistics
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import malaga_checkpoints as mc  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--evidence", type=Path, required=True)
    ap.add_argument("--tum", type=Path, required=True)
    args = ap.parse_args()
    sig = mc.load(args.evidence)
    gt = mc.tum(args.tum)
    gps = [(t, *mc.enu(la, lo)) for t, la, lo, _, _ in mc.pairs(sig["gps_normal.latitude"], sig["gps_normal.longitude"])]

    def residuals(lag: float) -> list[tuple[float, float]]:
        out = []
        for t, e, n in gps:
            ref = mc.nearest(gt, t - lag, 0.05)
            if ref:
                out.append((e - ref[1], n - ref[2]))
        return out

    for lag in (0.0, 0.5, 1.0, 2.0, 3.0, 5.0):
        r = residuals(lag)
        dx, dy = statistics.mean(a for a, _ in r), statistics.mean(b for _, b in r)
        res = sorted(math.hypot(a - dx, b - dy) for a, b in r)
        print(f"lag {lag:3.1f} s  n={len(r)}  best constant offset ({dx:7.2f}, {dy:7.2f}) m  "
              f"residual median {statistics.median(res):5.2f} m  p90 {res[int(0.9 * len(res))]:5.2f} m")
    r = residuals(0.0)
    for i in range(0, len(r), 40):
        print(f"sample {i:3}: raw offset east {r[i][0]:8.2f} m  north {r[i][1]:8.2f} m")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
