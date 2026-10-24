"""Smoke-test an *installed* openmhs Python package against a real gateway.

Run it with the Python of a clean virtual environment that has only the built wheel or
sdist installed (the repository's python/ directory must not be importable), for example:

    python -m venv .venv && .venv/bin/pip install dist/openmhs-0.1.0-py3-none-any.whl
    .venv/bin/python -I tools/smoke_python_package.py --bin path/to/openmhs

The repository's own tests (python/tests) put the source tree on sys.path, so they cannot
show that the packaged files work. This does: it starts `openmhs serve --sim`, then uses
only the installed package to grant, authenticate, get refused, write and read back.
Simulated devices only.
"""
import argparse
import os
import pathlib
import subprocess
import sys
import tempfile
import time


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True, help="path to the openmhs binary")
    ap.add_argument("--allow-source-tree", action="store_true", help="skip the installed-package check")
    args = ap.parse_args()

    import openmhs
    from openmhs import Client, Quantity

    here = pathlib.Path(openmhs.__file__).resolve()
    if not args.allow_source_tree and "site-packages" not in here.parts:
        sys.exit(f"openmhs was imported from {here}, not from an installed package")
    print(f"openmhs {openmhs.__version__} from {here.parent}")
    assert openmhs.__version__.count(".") >= 2
    assert (here.parent / "py.typed").exists(), "py.typed is missing from the package"

    binary = str(pathlib.Path(args.bin).resolve())
    with tempfile.TemporaryDirectory() as tmp:
        home = pathlib.Path(tmp) / "home"
        env = dict(os.environ, OPENMHS_HOME=str(home))
        gateway = subprocess.Popen([binary, "serve", "--sim", "--port", "0"], env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            deadline = time.time() + 30
            operator = None
            while time.time() < deadline:
                try:
                    operator = Client.from_home(home)
                    operator.info()
                    break
                except Exception:
                    operator = None
                    time.sleep(0.3)
            assert operator is not None, "the gateway did not start"

            info = operator.info()
            ids = sorted(d.device["id"] for d in operator.devices())
            print("gateway", info.version, "devices", ids)
            assert ids == ["sim.axis-01", "sim.psu-01", "sim.thermo-01"], ids

            operator.add_grant({
                "id": "agent-psu", "principal": "agent:smoke", "device": "sim.psu-01",
                "capability": "*", "operations": ["read", "write"], "effect": "allow",
            })
            agent = Client.from_home(home, token=operator.create_token("agent:smoke", any_scope=True).token)

            too_fast = agent.write("sim.psu-01", "power.output.voltage.setpoint", Quantity(5, "V"))
            assert too_fast.denied, too_fast
            assert [r.code for r in too_fast.reasons] == ["command.slew_exceeded"], too_fast.reasons
            print("5 V   ->", too_fast.state.value if hasattr(too_fast.state, "value") else too_fast.state, "command.slew_exceeded")

            ok = agent.write("sim.psu-01", "power.output.voltage.setpoint", Quantity(2, "V"))
            assert ok.confirmed, ok
            print("2 V   ->", ok.state.value if hasattr(ok.state, "value") else ok.state, "(confirmed by readback)")

            reading = agent.read("sim.psu-01", "power.output.voltage.setpoint")
            assert reading.usable, reading
            print("read  ->", reading.state.value)

            out_of_range = agent.write("sim.psu-01", "power.output.voltage.setpoint", Quantity(48, "V"))
            assert [r.code for r in out_of_range.reasons] == ["command.value.out_of_range"], out_of_range.reasons

            verdict = operator.audit_verify()
            assert verdict.valid, verdict
            print("audit ->", verdict.records, "records, chain valid")
        finally:
            gateway.kill()
            gateway.wait()
    print("ok")


if __name__ == "__main__":
    main()
