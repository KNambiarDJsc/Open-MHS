# Governance

OpenMHS is maintainer-led. This page says how decisions are made today, honestly, so
that contributors know what to expect.

## Roles

- **Maintainer.** Currently one person ([@KNambiarDJsc](https://github.com/KNambiarDJsc)).
  Merges changes, cuts releases, and has the last word on scope, the specification and
  the safety model.
- **Contributor.** Anyone who opens an issue, a pull request, an adapter or an RFC.
- **Adapter maintainer.** Whoever maintains an entry in the [adapter registry](registry/README.md).
  They are responsible for their adapter, not for OpenMHS. Registry categories describe
  evidence, not endorsement.

More maintainers are welcome after sustained, careful contributions. The maintainer
invites them; there is no vote.

## How decisions are made

1. **Everyday changes:** a pull request, review, merge. The maintainer merges when CI is
   green and the change fits the project.
2. **Specification, adapter contract, API, security model:** an
   [RFC](docs/rfcs/0000-template.md) first, discussed in public on its pull request.
   The maintainer decides after discussion and records lasting decisions as an ADR in
   [docs/adr/](docs/adr/README.md).
3. **Disagreement:** the maintainer decides and writes down why. If the project grows a
   second maintainer, decisions that cannot reach consensus are made by the maintainers
   together, and this page will be updated.

## Principles that decisions follow

These are fixed unless an RFC changes them, and the bar is high:

- Deterministic policy decides what an agent may do; a model never does.
- Hardware safety systems keep final authority. OpenMHS is not a safety function.
- Claims need evidence. Simulated is never reported as physically validated.
- The project stays independent and does not claim compatibility with standards whose
  specifications are not public.
- Minimal dependencies; no hand-written cryptography.

## Releases

The maintainer cuts releases from tagged commits using the process in
[docs/release/RELEASING.md](docs/release/RELEASING.md). Versions follow Semantic Versioning
once 1.0 ships; before then, minor versions may break the adapter contract and the
specification, and the [CHANGELOG](CHANGELOG.md) says so.

## Changing this document

By pull request, decided by the maintainer.
