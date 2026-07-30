# Example: Sphere Packing (Verso blueprint)

A real, Mathlib-backed research blueprint: the formalization of the sphere
packing problem (Cohn–Elkies, E8, Viazovska-style modular forms machinery).

- **Blueprint overlay repo:** [`ejgallego/verso-sphere-packing`](https://github.com/ejgallego/verso-sphere-packing) @ `4cbb43d`
- **Upstream formalization (the "real Lean repo"):** [`thefundamentaltheor3m/Sphere-Packing-Lean`](https://github.com/thefundamentaltheor3m/Sphere-Packing-Lean) @ `1828993`
- **Rendered blueprint:** [spherepackingblueprint](https://leanprover.github.io/verso-blueprint/reference-blueprints/v4.31.0/spherepackingblueprint/)

| | |
|---|---|
| Lean toolchain | `v4.31.0` |
| verso-blueprint | `v4.31.0` |
| Mathlib | `v4.31.0` |
| Render entrypoint | `SpherePackingBlueprintMain.lean` |
| Math library | `SpherePacking` |
| probe-lean | `probe-lean-v4.31.0` (0.9.5) |

## The verilib scenario

This example assumes verilib holds **only the Lean repo** — no pre-rendered
manifest. `probe-leanblueprint` (driven here by
[`../reproduce-verso-example.sh`](../reproduce-verso-example.sh)) does everything
from the repo: build, render the blueprint, run `probe-lean` for the atom base,
then join.

```bash
../reproduce-verso-example.sh \
  --repo   https://github.com/ejgallego/verso-sphere-packing \
  --render SpherePackingBlueprintMain.lean \
  --sub    Sphere-Packing-LaTeX-Reference \
  --lib    SpherePacking \
  --probe-lean probe-lean-v4.31.0
```

`probe-lean` extracted **1465 atoms** from the `SpherePacking` library and flagged
**60 sorries** independently of the blueprint.

## Result

Full report: [`blueprint-stats.txt`](./blueprint-stats.txt) · machine summary:
[`extract.summary.json`](./extract.summary.json).

```
Headline: 21/106 theorems probe-lean-confirmed fully proved (19.8%)
  (blueprint claims 24/106; 3 not backed by probe-lean's verification status)
Blueprint nodes: 140  (bound 75 · planned-only 51 · decl-missing 14 · partial-missing 1 · mismatches 0)
```

- **75 of 89** statement-formalized nodes bound to a real `probe-lean` atom.
- **14 decl-missing**: blueprint nodes whose declaration is not in the
  `SpherePacking` library (modular-forms scaffolding still living in Mathlib or
  not yet formalized locally, e.g. `«def:Schwartz-Space»`, `«def:dedekind_eta»`).

## Comparison against Verso's own rendering

The rendered site derives its progress bars from the same
`blueprint-manifest.json`. probe-leanblueprint ingests that manifest and its
axes reproduce the manifest's raw status tallies exactly, under the documented
mapping:

| Verso `proofStatus` | count | probe-leanblueprint proof axis | count |
|---|---|---|---|
| `formalizedWithAncestors` | 42 | `fully-proved` | 42 |
| `formalized` | 38 | `proved` | 38 |
| `ready` | 18 | `ready` | 18 |
| `none` (33) + `incomplete` (9) | 42 | `none` | 42 |

| Verso `statementStatus` | count | probe statement axis | count |
|---|---|---|---|
| `formalized` | 89 | `formalized` | 89 |
| `ready` | 22 | `ready` | 22 |
| `blocked` | 29 | `blocked` | 29 |

**What probe-leanblueprint adds** over the rendered page: the join to
`probe-lean` atoms (`bound` / `decl-missing`) and a cross-check of the
blueprint's claimed proof status against `probe-lean`'s machine verification —
here **0 mismatches**, i.e. every "formalized"/"fully-proved" claim in the
blueprint agrees with the sorry-free status probe-lean computed.
