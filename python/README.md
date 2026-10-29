# openmhs (Python client)

A Python client for the OpenMHS gateway's local API ([spec/api.md](https://github.com/KNambiarDJsc/Open-MHS/blob/main/spec/api.md)).
Standard library only, Python 3.10+. **Not on PyPI yet**: install it from a checkout of the repository. It talks to a running
OpenMHS gateway (`openmhs serve`); it does not include one.

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

For a command whose HTTP response might be lost, `execute` creates/reuses a stable
idempotency key and returns a handle. The request is submitted once; the handle looks up the
journal by `(device, key)` and never resends it:

```python
request = {
    "target": {"device": "sim.psu-01", "capability": "power.output.voltage.setpoint"},
    "operation": "write",
    "value": {"value": 2, "unit": "V"},
}
run = gw.execute(request, idempotency_key="workflow-42-step-3", timeout=10)
receipt = run.wait(timeout=10)       # bounded status polling; None means not found yet
if run.action_required == "reconcile":
    receipt = run.reconcile()        # explicit readback; never a resend
```

Pass a caller-persisted `idempotency_key` if the caller may restart before it can retrieve
the receipt. `timeout` bounds the submit HTTP call; `wait(timeout=...)` separately bounds
status polling. When the SDK generates a key, persist `run.idempotency_key` for later lookup.
`run.action_required` distinguishes status lookup, approval, reconciliation and human
verification. A missing receipt after a transport error means admission is not yet known;
check again with `run.refresh()` when the gateway is reachable, and do not submit a new key.

- Exceptions (`GatewayError` with a stable `.code`, `UnauthorizedError`,
  `NotRunningError`, `GatewayUnreachableError`) are for requests that could not become
  a command at all.
- The models (`Receipt`, `ReadResult`, `PreflightResult`, …) mirror `spec/schemas`.
  Tests check their fields and required fields against the schemas, and check that the
  spec's examples and live gateway responses round-trip unchanged. Fields from a newer
  gateway are kept in `.extra`.
- Leases (`acquire_lease`, `renew_lease`, `release_lease`) and plans (`preflight_plan`,
  `submit_plan`, `plan`, `plans`, `cancel_plan`, and `decide_plan` for a human approver)
  follow [spec/topology.md](https://github.com/KNambiarDJsc/Open-MHS/blob/main/spec/topology.md) and [spec/plans.md](https://github.com/KNambiarDJsc/Open-MHS/blob/main/spec/plans.md).
- `Client` never prints its token, and is safe to share between threads.

Tests run a real gateway binary:

```sh
cargo build -p openmhs-cli
python -m unittest discover -s python/tests     # jsonschema optional, for the schema tests
```

Independent project; not affiliated with Anthropic.
