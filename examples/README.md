# Examples

Curated, real blueprint projects that `probe-leanblueprint` is known to run on,
each captured with:

- a link to the upstream GitHub repo (and its **rendered** blueprint site),
- the exact versions/commands used to reproduce it,
- the tool's summary (`extract.summary.json`),
- the `scripts/blueprint_stats.py` report (`blueprint-stats.txt`), and
- a side-by-side comparison against the numbers the blueprint's **own** Verso
  rendering reports, so the tool's output can be trusted against an independent
  source.

## Index

| Example | Upstream Lean repo | verso-blueprint | Lean | Bound / formalized | Headline (machine-confirmed) |
|---------|--------------------|-----------------|------|--------------------|------------------------------|
| [`verso-carleson`](./verso-carleson/) | [`fpvandoorn/carleson`](https://github.com/fpvandoorn/carleson) ([rendered](https://leanprover.github.io/verso-blueprint/reference-blueprints/v4.31.0/verso-carleson/)) | v4.31.0 | v4.31.0 | 152 / 160 | 146/161 (90.7%) · claims 154 |
| [`verso-noperthedron`](./verso-noperthedron/) | [`jcreedcmu/Noperthedron`](https://github.com/jcreedcmu/Noperthedron) ([rendered](https://ejgallego.github.io/verso-noperthedron/)) | v4.32.0 | v4.32.0-rc1 | 58 / 59 | 37/59 (62.7%) |
| [`verso-flt`](./verso-flt/) | [`ImperialCollegeLondon/FLT`](https://github.com/ImperialCollegeLondon/FLT) ([rendered](https://leanprover.github.io/verso-blueprint/reference-blueprints/v4.32.0/verso-flt/)) | v4.32.0 | v4.32.0-rc1 | 150 / 171 | 88/194 (45.4%) · claims 105 |
| [`verso-sphere-packing`](./verso-sphere-packing/) | [`thefundamentaltheor3m/Sphere-Packing-Lean`](https://github.com/thefundamentaltheor3m/Sphere-Packing-Lean) ([rendered](https://leanprover.github.io/verso-blueprint/reference-blueprints/v4.31.0/spherepackingblueprint/)) | v4.31.0 | v4.31.0 | 75 / 89 | 21/106 (19.8%) · claims 24 |
| [`verso-blueprint-project-template`](./verso-blueprint-project-template/) | [verso-blueprint `project_template`](https://github.com/leanprover/verso-blueprint/tree/v4.31.0/project_template) | v4.31.0 | v4.31.0 | 4 / 6 (toy) | 0/5 (0.0%) · claims 1 |

**Headline** is theorems *machine-confirmed* fully proved — the blueprint claims
`fully_proved` **and** `probe-lean` backs it (bound to a present atom, not
contradicted). `claims N` shows the blueprint's own (higher) count where
decl-missing or contradicted nodes exist; the per-example READMEs break this
down. Every real example has **0 status mismatches** (no node the blueprint
claims proved is contradicted by `probe-lean`); the gap to `claims` here is
entirely decl-missing nodes (decls that live upstream in Mathlib, outside the
project's own atom base).

## Reading the totals

Each `extract.summary.json` reports a `totals` block. Every blueprint node lands
in exactly one of three buckets, and they sum to the node count:

```
nodes = with-lean-decl + decl-missing + planned-only
```

| Field | Meaning |
|-------|---------|
| **`with-lean-decl`** (`bound` in `blueprint_stats.py`) | Node references ≥1 Lean declaration **and at least one exists** in the probe-lean atom base → the join succeeded. |
| **`decl-missing`** | Node references Lean decl(s) but **none** are in the atom base (decl lives in Mathlib, isn't formalized in this library, or the render was against a different commit). |
| **`planned-only`** | Node has **no** Lean declaration at all — a roadmap / informal node. |
| **`mismatches`** | Bound nodes whose blueprint proof claim disagrees with probe-lean's machine (sorry) status. |

So e.g. **Carleson `nodes 161 / with-lean-decl 152`** means 161 blueprint nodes,
152 of them joined to a real Lean declaration (~94% coverage; 8 `decl-missing`,
1 `planned-only`) — a near-fully-formalized project. **FLT `245 / 150`** is a
different shape: 73 of the non-bound nodes are `planned-only` (a large,
not-yet-formalized roadmap) and 22 are `decl-missing`.

## The verilib scenario

The four real examples above are run as if verilib holds **only the upstream
Lean repo** — no pre-rendered manifest, no pre-extracted atoms. The single
driver script [`reproduce-verso-example.sh`](./reproduce-verso-example.sh) does
the whole pipeline from the repo:

1. clone the blueprint overlay repo (+ its math-library submodule),
2. `lake exe cache get` + `lake build`,
3. render the blueprint (`lake env lean --run <Main>.lean --output _out/site`),
4. `probe-lean extract` on the **math submodule** for the atom base,
5. `probe-leanblueprint extract … --lean <atoms> --verso-manifest _out/site`.

Each example's `README.md` gives the exact one-liner. Because these are all
Mathlib-backed (and FLT/Carleson are large), the multi-MB manifests and atom
extracts are **not** committed — only the ~2 KB `extract.summary.json` sidecar
and the `blueprint-stats.txt` report, both regenerable via the script.

### probe-lean toolchain matching

`probe-lean` reads compiled `.olean` files and must be built for the **same**
Lean toolchain as the target project. Build one per toolchain and put it on
`PATH` (e.g. `probe-lean-v4.31.0`, `probe-lean-v4.32.0-rc1`); pass the right one
via `--probe-lean`.

## <a id="atom-keyed-node-collapse"></a>Same-decl collisions (shadow atoms)

The enriched `extract.json` is keyed by **Lean declaration** (`probe:<decl>`),
not by blueprint node. When two distinct blueprint nodes resolve to the *same*
Lean declaration, only one can own that atom (keep-last), so the tool preserves
the other as a **`blueprint-shadow`** synthetic atom. This keeps the extract
**node-complete**: every blueprint node is represented, and the summary sidecar,
the `extract.json`, and `blueprint_stats.py` all agree.

| Example | manifest nodes | `with-lean-decl` (bound) | collisions | shadow atoms |
|---------|----------------|--------------------------|------------|--------------|
| Sphere Packing | 140 | 75 | 0 | 0 |
| Noperthedron | 68 | 58 | 0 | 0 |
| Carleson | 161 | 152 | 2 | 2 |
| FLT | 245 | 150 | 4 | 3 |

`blueprint_stats.py` counts a shadow atom as `bound` (its node genuinely binds a
present decl), so `nodes` and `bound` match the summary sidecar on every example.
A shadow retains the node's own status and its `blueprint-status-mismatch`
signal, so nothing is silently dropped. Collisions are also emitted as warnings
on stderr (`atom … is bound by multiple blueprint nodes …`). FLT has 4 collision
*events* but 3 shadows, because one loser co-owns another present decl and is
represented through that atom instead of a shadow.

> This only surfaces with a real atom base; with an empty atom stub every node
> falls through to `decl-missing` and there are no collisions.

## Compatibility note (Verso ecosystem)

The Verso adapter consumes the **`blueprint-manifest.json`** with a populated
`graphs[].nodes[]` array carrying per-node `statementStatus` / `proofStatus`.

| verso-blueprint | manifest file | `graphs` populated? | Works? |
|-----------------|---------------|---------------------|--------|
| **v4.28.0** | `blueprint-preview-manifest.json` | no (preview-only schema) | ❌ 0 nodes |
| **v4.29.0 – v4.32.0** | `blueprint-manifest.json` | yes | ✅ |

`v4.31.0`+ introduced two status values the adapter maps: `mathlib` (statement
already upstream in Mathlib → `formalized`) and `incomplete` (Lean proof present
but containing `sorry` → not-proved `none`). See `CHANGELOG.md`.
