"""Serial dilution on an Opentrons Flex: reviewed protocol for the OpenMHS Opentrons adapter.

The code is fixed and pinned by its SHA-256 in the adapter's configuration. The adapter
calls ``dilute(protocol, params)`` with parameters from a request OpenMHS has already
validated, so an agent chooses numbers, never code. (opentrons 10.1.0 accepts neither
bundled data files nor runtime-parameter overrides for API levels the Flex needs, so the
documented interactive context, ``opentrons.simulate.get_protocol_api``, is the way in.)
``run`` uses the defaults, so ``opentrons_simulate serial_dilution.py`` works on its own.

Layout: stock in reservoir well A1 and diluent in A2; a 96-well plate. The first well of
row A gets stock; every later well gets diluent, then the transfer from the well before
it, mixed. One fresh tip per transfer. (opentrons 10.x simulates the Flex only; OT-2
protocols are refused by its simulator.)
"""

from opentrons import protocol_api

metadata = {"protocolName": "Serial dilution (OpenMHS reviewed)", "author": "OpenMHS research"}
requirements = {"robotType": "Flex", "apiLevel": "2.20"}


DEFAULTS = {"n_wells": 6, "transfer_ul": 50.0, "diluent_ul": 100.0, "mix_reps": 3}


def run(protocol: protocol_api.ProtocolContext) -> None:
    dilute(protocol, DEFAULTS)


def dilute(protocol: protocol_api.ProtocolContext, p: dict) -> None:
    n, transfer_ul, diluent_ul, mix_reps = p["n_wells"], p["transfer_ul"], p["diluent_ul"], p["mix_reps"]

    tips = protocol.load_labware("opentrons_flex_96_tiprack_1000ul", "C1")
    reservoir = protocol.load_labware("nest_12_reservoir_15ml", "C2")
    plate = protocol.load_labware("nest_96_wellplate_200ul_flat", "C3")
    protocol.load_trash_bin("A3")
    pipette = protocol.load_instrument("flex_1channel_1000", "right", tip_racks=[tips])

    row = plate.rows()[0][:n]
    pipette.transfer(transfer_ul + diluent_ul, reservoir["A1"], row[0])
    pipette.transfer(diluent_ul, reservoir["A2"], row[1:])
    for src, dst in zip(row[:-1], row[1:]):
        pipette.transfer(transfer_ul, src, dst, mix_after=(mix_reps, transfer_ul) if mix_reps else None)
