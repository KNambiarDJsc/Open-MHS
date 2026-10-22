# Naming check (2026-10-23)

## "OpenMHS" is crowded

Repositories already using the name or a near-identical one:

- `Abenor-Labs/Open-MHS` — MHS-inspired safety middleware (Apache-2.0)
- `vraj130/open-mhs`, `SCUT-ESA/open-mhs`, `tongriyaotxt/open-mhs` — MHS-inspired
- `Pratyush-exe/OpenMHS` — "unofficial shared spec sketch"
- `daviddaiiiii/OpenMHS` — unrelated (OpenFlexure microscope tooling, GPL-3.0)

Sources: [awesome-mhs](https://github.com/carllippert/awesome-mhs), GitHub search.

The name also ties the project's identity to Anthropic's MHS, which (a) we cannot
claim compatibility with and (b) could create trademark/confusion risk if
"MHS" becomes an Anthropic mark.

## Registry availability (HTTP 404 = free)

| Name | crates.io | PyPI | GitHub user/org |
|---|---|---|---|
| openmhs | free | free | **taken** |
| open-mhs | free | free | free |
| mhs-gateway | free | free | free |
| physgate | free | free | free |
| hwgate | free | free | taken |
| actuon | free | free | free |
| agentinterlock | free | free | free |
| interlock | taken | free | taken |
| armature / actuate / gantry / cordon | taken | taken | taken |

GitHub org availability only matters if we want a dedicated org; the repo can live
under a personal account. Domain and trademark searches are **not done** — do them
before any public launch.

## Recommendation

1. **Do not launch publicly as "OpenMHS".** At least six repos share the name, and
   it positions us as one more MHS clone — the opposite of the differentiation in
   [landscape.md](landscape.md).
2. Pick a distinct name **before M1**, because crate names (`<name>-types`,
   `<name>-engine`, …) and the CLI binary name are baked in from M1 onward.
3. Candidates fully free on all three checked registries: **`physgate`**,
   **`actuon`**, `agentinterlock`, `mhs-gateway` (still MHS-tied).
   `physgate` says what it is (a gate between agents and the physical world).

Decision belongs to the project owner. Until decided, docs use "OpenMHS".

## Decision (2026-10-23)

The project owner chose to **keep "OpenMHS"**, knowing about the collisions above. The
consequences:

- Crates and binaries use the `openmhs` prefix.
- Public material must say clearly that this project is independent of Anthropic and of
  the other "OpenMHS" repositories.
- Domain and trademark checks are still required before any public launch.
