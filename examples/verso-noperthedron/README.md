# Example: Noperthedron (Verso blueprint)

A real, completed research blueprint: the "Noperthedron" construction (a
point-symmetric polyhedron that is not Rupert), ported to Verso from the
upstream LaTeX blueprint.

- **Blueprint overlay repo:** [`ejgallego/verso-noperthedron`](https://github.com/ejgallego/verso-noperthedron) @ `ad062f0`
- **Upstream formalization (the "real Lean repo"):** [`jcreedcmu/Noperthedron`](https://github.com/jcreedcmu/Noperthedron) @ `8350205`
- **Rendered blueprint:** [ejgallego.github.io/verso-noperthedron](https://ejgallego.github.io/verso-noperthedron/) · [reference mirror](https://leanprover.github.io/verso-blueprint/reference-blueprints/v4.32.0/noperthedron/)

| | |
|---|---|
| Lean toolchain | `v4.32.0-rc1` |
| verso-blueprint | `v4.32.0` |
| Mathlib | `ba1e3bb` (pinned commit) |
| Render entrypoint | `Main.lean` |
| Math library | `Noperthedron` |
| probe-lean | `probe-lean-v4.32.0-rc1` |

> **Git LFS note:** the `Noperthedron` submodule carries a 30 MB LFS data blob
> (`solution_tree_v6.zip`) whose upstream LFS budget is currently exhausted. It
> is not needed to build the `Noperthedron` library, so the clone uses
> `--skip-lfs`.

## The verilib scenario

verilib holds **only the Lean repo**; `probe-leanblueprint` (via
[`../reproduce-verso-example.sh`](../reproduce-verso-example.sh)) builds, renders,
runs `probe-lean`, and joins:

```bash
../reproduce-verso-example.sh \
  --repo   https://github.com/ejgallego/verso-noperthedron \
  --render Main.lean \
  --sub    Noperthedron \
  --lib    Noperthedron \
  --probe-lean probe-lean-v4.32.0-rc1 \
  --skip-lfs
```

`probe-lean` extracted **1694 atoms** and found **0 sorries** — the upstream
formalization is complete.

## Result

Full report: [`blueprint-stats.txt`](./blueprint-stats.txt) · machine summary:
[`extract.summary.json`](./extract.summary.json).

```
Headline: 37/59 theorems fully proved (62.7%)
Blueprint nodes: 68  (bound 58 · planned-only 9 · decl-missing 1 · mismatches 0)
```

- **58 of 59** statement-formalized nodes bound to a real `probe-lean` atom —
  near-total coverage; only **1 decl-missing**.

## Comparison against Verso's own rendering

| Verso `proofStatus` | count | probe-leanblueprint proof axis | count |
|---|---|---|---|
| `formalizedWithAncestors` | 44 | `fully-proved` | 44 |
| `formalized` | 15 | `proved` | 15 |
| `ready` | 7 | `ready` | 7 |
| `none` | 2 | `none` | 2 |

| Verso `statementStatus` | count | probe statement axis | count |
|---|---|---|---|
| `formalized` | 59 | `formalized` | 59 |
| `ready` | 9 | `ready` | 9 |

**What probe-leanblueprint adds:** the atom join (58 `bound`, 1 `decl-missing`)
and the machine cross-check — **0 mismatches** between the blueprint's proof
claims and probe-lean's sorry-free verification.
