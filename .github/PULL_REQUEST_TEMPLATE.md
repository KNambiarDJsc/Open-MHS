## What and why

<!-- What does this change, and why? Link the issue or RFC. -->

## How it was tested

<!-- Commands you ran and what you saw. Say whether it was simulated or physical hardware.
     Never paste output you did not get. -->

## Checklist

- [ ] `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass
- [ ] Tests added or updated for changed behaviour
- [ ] Docs and `spec/` examples updated where affected
- [ ] No security test was weakened, and no real hardware is needed to run the tests
- [ ] Claims in docs match what was tested (simulated is not physical)
- [ ] New dependencies, if any, are justified and allowed by `deny.toml`
