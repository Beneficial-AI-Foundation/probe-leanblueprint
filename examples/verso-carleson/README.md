# Example: Carleson (Verso blueprint)

A real, nearly-complete research blueprint: the formalization of Carleson's
theorem on pointwise convergence of Fourier series (the metric-space Carleson
project).

- **Blueprint overlay repo:** [`ejgallego/verso-carleson`](https://github.com/ejgallego/verso-carleson) @ `5b34f5d`
- **Upstream formalization (the "real Lean repo"):** [`fpvandoorn/carleson`](https://github.com/fpvandoorn/carleson) @ `8e93bee`
- **Rendered blueprint:** [verso-carleson](https://leanprover.github.io/verso-blueprint/reference-blueprints/v4.31.0/verso-carleson/)

| | |
|---|---|
| Lean toolchain | `v4.31.0` |
| verso-blueprint | `v4.31.0` |
| Mathlib | via `Carleson`'s `lake-manifest` |
| Render entrypoint | `BlueprintMain.lean` |
| Math library | `Carleson` |
| probe-lean | `probe-lean-v4.31.0` (0.9.5) |

## The verilib scenario

verilib holds **only the Lean repo**; `probe-leanblueprint` (via
[`../reproduce-verso-example.sh`](../reproduce-verso-example.sh)) builds, renders,
runs `probe-lean`, and joins:

```bash
../reproduce-verso-example.sh \
  --repo   https://github.com/ejgallego/verso-carleson \
  --render BlueprintMain.lean \
  --sub    Carleson \
  --lib    Carleson \
  --probe-lean probe-lean-v4.31.0
```

`probe-lean` extracted **3338 atoms** from the `Carleson` library and flagged
**5 sorries**.

## Result

Full report: [`blueprint-stats.txt`](./blueprint-stats.txt) · machine summary:
[`extract.summary.json`](./extract.summary.json).

```
Headline: 146/161 theorems probe-lean-confirmed fully proved (90.7%)
  (blueprint claims 154/161; 8 not backed by probe-lean's verification status)
Blueprint nodes: 161   (bound 152 · planned-only 1 · decl-missing 8 · partial-missing 0 · mismatches 0)
```

- **152 of 160** statement-formalized nodes bound to a real `probe-lean` atom —
  the most complete of the four examples, at ~95%.

## Comparison against Verso's own rendering

All **161** graph nodes are represented (the extract is node-complete). Two
same-decl collisions — `«convergence-for-twice-contdiff»` (shares a decl with
`«convergence-for-smooth»`) and `«partial-Fourier-sums-of-small»` (shares one
with `«control-approximation-effect»`) — are preserved as `blueprint-shadow`
atoms, so the axis tallies reproduce the manifest's raw status counts exactly.
See [`../README.md`](../README.md#atom-keyed-node-collapse) for details.

| Verso `proofStatus` | count | probe proof axis | count |
|---|---|---|---|
| `formalizedWithAncestors` | 154 | `fully-proved` | 154 |
| `formalized` | 6 | `proved` | 6 |
| `none` | 1 | `none` | 1 |

| Verso `statementStatus` | count | probe statement axis | count |
|---|---|---|---|
| `formalized` | 160 | `formalized` | 160 |
| `blocked` | 1 | `blocked` | 1 |

**What probe-leanblueprint adds:** the atom join (152 `bound`, 8 `decl-missing`)
and the machine cross-check — **0 mismatches** between the blueprint's proof
claims and probe-lean's sorry detection.
