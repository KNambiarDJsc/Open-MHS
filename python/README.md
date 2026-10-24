# openmhs (Python client)

A Python client for the OpenMHS gateway's local API ([spec/api.md](../spec/api.md)).
Standard library only, Python 3.10+. **Not published**: install it from this repository.

```sh
pip install -e python          # or put python/ on PYTHONPATH
```

```python
from openmhs import Client, Quantity

gw = Client.from_home(token="<agent token>")      # finds the running gateway
r = gw.write("sim.psu-01", "power.output.voltage.setpoint", Quantity(2, "V"),
             idempotency_key="step-1")
if r.confirmed:                                    # verified by reading the device back
    print("done")
elif r.denied:                                     # a refusal is a receipt, not an exception
    for reason in r.reasons:
        print(reason.code, reason.details)
```

- Exceptions (`GatewayError` with a stable `.code`, `UnauthorizedError`,
  `NotRunningError`, `GatewayUnreachableError`) are for requests that could not become
  a command at all.
- The models (`Receipt`, `ReadResult`, `PreflightResult`, …) mirror `spec/schemas`.
  Tests check their fields and required fields against the schemas, and check that the
  spec's examples and live gateway responses round-trip unchanged. Fields from a newer
  gateway are kept in `.extra`.
- Leases (`acquire_lease`, `renew_lease`, `release_lease`) and plans (`preflight_plan`,
  `submit_plan`, `plan`, `plans`, `cancel_plan`, and `decide_plan` for a human approver)
  follow [spec/topology.md](../spec/topology.md) and [spec/plans.md](../spec/plans.md).
- `Client` never prints its token, and is safe to share between threads.

Tests run a real gateway binary:

```sh
cargo build -p openmhs-cli
python -m unittest discover -s python/tests     # jsonschema optional, for the schema tests
```

Independent project; not affiliated with Anthropic.
