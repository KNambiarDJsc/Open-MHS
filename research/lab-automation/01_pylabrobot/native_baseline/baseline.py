"""PLR native baseline (no OpenMHS): one tip, one aspirate, one dispense, tip returned.

Runs PyLabRobot's own device-free backend (LiquidHandlerChatterboxBackend), which prints
each operation and models no physics. Every volume and tip state below comes from
PyLabRobot's software trackers (resources/volume_tracker.py, resources/tip_tracker.py),
not from any measurement.

    python baseline.py            # prints a JSON record of the run on stdout
"""

from __future__ import annotations

import asyncio
import contextlib
import io
import json
import sys

from pylabrobot import __version__ as PLR_VERSION
from pylabrobot.liquid_handling import LiquidHandler
from pylabrobot.liquid_handling.backends.chatterbox import LiquidHandlerChatterboxBackend
from pylabrobot.resources import (
    PLT_CAR_L5AC_A00,
    TIP_CAR_480_A00,
    cor_96_wellplate_360uL_Fb,
    hamilton_96_tiprack_300uL,
    set_tip_tracking,
    set_volume_tracking,
)
from pylabrobot.resources.hamilton import STARLetDeck

SOURCE_UL = 200.0
TRANSFER_UL = 50.0


def snapshot(source, dest, tips) -> dict:
    return {
        "source_A1_ul": source.get_well("A1").tracker.get_used_volume(),
        "dest_B1_ul": dest.get_well("B1").tracker.get_used_volume(),
        "tip_A1_present": tips.get_item("A1").tracker.has_tip,
    }


async def main() -> dict:
    set_tip_tracking(True)
    set_volume_tracking(True)
    deck = STARLetDeck()
    lh = LiquidHandler(backend=LiquidHandlerChatterboxBackend(num_channels=1), deck=deck)
    tips = hamilton_96_tiprack_300uL(name="tips")
    source = cor_96_wellplate_360uL_Fb(name="source")
    dest = cor_96_wellplate_360uL_Fb(name="dest")
    tip_car = TIP_CAR_480_A00(name="tip_carrier")
    tip_car[0] = tips
    plate_car = PLT_CAR_L5AC_A00(name="plate_carrier")
    plate_car[0] = source
    plate_car[1] = dest
    deck.assign_child_resource(tip_car, rails=1)
    deck.assign_child_resource(plate_car, rails=10)

    chatter = io.StringIO()
    with contextlib.redirect_stdout(chatter):
        await lh.setup()
        source.get_well("A1").tracker.set_volume(SOURCE_UL)
        before = snapshot(source, dest, tips)
        await lh.pick_up_tips(tips["A1"])
        await lh.aspirate(source["A1"], vols=[TRANSFER_UL])
        await lh.dispense(dest["B1"], vols=[TRANSFER_UL])
        await lh.return_tips()
        after = snapshot(source, dest, tips)
        await lh.stop()
    return {
        "pylabrobot": PLR_VERSION,
        "python": sys.version.split()[0],
        "backend": "LiquidHandlerChatterboxBackend",
        "before": before,
        "after": after,
        "backend_log_lines": [l for l in chatter.getvalue().splitlines() if l.strip()],
        "evidence": "software trackers only; the chatterbox backend models no liquid",
    }


if __name__ == "__main__":
    print(json.dumps(asyncio.run(main()), indent=2))
