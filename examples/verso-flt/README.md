# Example: Fermat's Last Theorem (Verso blueprint)

A real, large, actively-developed research blueprint: the Imperial College
London formalization of Fermat's Last Theorem.

- **Blueprint overlay repo:** [`ejgallego/verso-flt`](https://github.com/ejgallego/verso-flt) @ `1695b7c`
- **Upstream formalization (the "real Lean repo"):** [`ImperialCollegeLondon/FLT`](https://github.com/ImperialCollegeLondon/FLT) @ `ee47fd2`
- **Rendered blueprint:** [verso-flt](https://leanprover.github.io/verso-blueprint/reference-blueprints/v4.32.0/verso-flt/)

| | |
|---|---|
| Lean toolchain | `v4.32.0-rc1` |
| verso-blueprint | `v4.32.0` |
| Mathlib | `a3364fa` (pinned commit) |
| Render entrypoint | `FLTBlueprintMain.lean` |
| Math library | `FLT` |
| probe-lean | `probe-lean-v4.32.0-rc1` |

## The verilib scenario

verilib holds **only the Lean repo**; `probe-leanblueprint` (via
[`../reproduce-verso-example.sh`](../reproduce-verso-example.sh)) builds, renders,
runs `probe-lean`, and joins:

```bash
../reproduce-verso-example.sh \
  --repo   https://github.com/ejgallego/verso-flt \
  --render FLTBlueprintMain.lean \
  --sub    FLT \
  --lib    FLT \
  --probe-lean probe-lean-v4.32.0-rc1
```

`probe-lean` extracted **4147 atoms** from the `FLT` library and found **0
sorries** — the currently-formalized parts are sorry-free; the blueprint's
open proofs are `ready`/`none` because they depend on statements not yet
formalized (`blocked`), not on `sorry`-ridden ones.

## Result

Full report: [`blueprint-stats.txt`](./blueprint-stats.txt) · machine summary:
[`extract.summary.json`](./extract.summary.json).

```
Headline: 105/194 theorems fully proved (54.1%)
Blueprint nodes: 245  (bound 150 · planned-only 73 · decl-missing 22 · mismatches 0)
```

- **150 of 171** statement-formalized nodes bound to a real `probe-lean` atom.
- **35 `blocked`** statements — FLT is mid-formalization, and the blueprint
  tracks a large frontier of not-yet-stated results.

## Comparison against Verso's own rendering

All **245** graph nodes are represented (the extract is node-complete). FLT has
4 same-decl collision events (e.g. `Wiles_Frey` / `Wiles_Frey_again` /
`hardly_ramified_reducible` all bind the same Frey-curve lemma); the losers are
preserved as `blueprint-shadow` atoms (or via a co-owned decl), so the axis
tallies reproduce the manifest's raw status counts exactly. See
[`../README.md`](../README.md#atom-keyed-node-collapse) for details.

| Verso `proofStatus` | count | probe proof axis | count |
|---|---|---|---|
| `formalizedWithAncestors` | 137 | `fully-proved` | 137 |
| `formalized` | 25 | `proved` | 25 |
| `ready` | 32 | `ready` | 32 |
| `none` (41) + `incomplete` (10) | 51 | `none` | 51 |

| Verso `statementStatus` | count | probe statement axis | count |
|---|---|---|---|
| `formalized` | 171 | `formalized` | 171 |
| `ready` | 39 | `ready` | 39 |
| `blocked` | 35 | `blocked` | 35 |

**What probe-leanblueprint adds:** the atom join (150 `bound`, 22 `decl-missing`)
and the machine cross-check — **0 mismatches** between the blueprint's proof
claims and probe-lean's sorry detection.
