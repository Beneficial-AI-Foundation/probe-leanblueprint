# Example: Verso Blueprint `project_template`

The canonical starter blueprint shipped by the Verso Blueprint project. It is
deliberately tiny (basic arithmetic facts on `Nat`, plus an intentionally
unfinished Collatz chapter), needs no Mathlib, and exercises every status the
two-axis model cares about — a good smoke-test that `probe-leanblueprint`
consumes a real `lake exe vbp build` manifest end-to-end.

## Upstream

- **Repo:** <https://github.com/leanprover/verso-blueprint/tree/v4.31.0/project_template>
- **Rendered blueprint site:** <https://leanprover.github.io/verso-blueprint/reference-blueprints/project-template/>

## Versions pinned for this capture

| Component | Version |
|-----------|---------|
| `verso-blueprint` | `v4.31.0` (commit `6561770257aaf21bcb0160f832153a79bb25e161`) |
| `verso` | `v4.31.0` |
| Lean toolchain | `leanprover/lean4:v4.31.0` |
| `blueprint-manifest.json` schema | `vbpInternalSchemaVersion: 3` |
| `probe-leanblueprint` | this repo (`feat/verso-auto-render`; accepts the `mathlib` / `incomplete` statuses) |

## Reproduce

Render the blueprint (this writes `blueprint-manifest.json`, committed here):

```bash
git clone --branch v4.31.0 --depth 1 https://github.com/leanprover/verso-blueprint
cd verso-blueprint/project_template
lake update
lake exe vbp build            # -> _out/site/html-multi/-verso-data/blueprint-manifest.json
```

Then run the tool. The canonical zero-config form (needs a `probe-lean` built
for Lean `v4.31.0` on `PATH`) is:

```bash
probe-leanblueprint extract .
python3 scripts/blueprint_stats.py .verilib/probes/leanblueprint_ProjectTemplate_0.0.0.json
```

The artifacts in this folder were produced by feeding the committed manifest
directly (equivalent join; the template's decl names are toolchain-independent):

```bash
probe-leanblueprint extract path/to/project_template \
    --adapter verso \
    --lean path/to/probe-lean-extract.json \
    --verso-manifest examples/verso-blueprint-project-template/blueprint-manifest.json
```

## Files

| File | What it is |
|------|-----------|
| `blueprint-manifest.json` | Input: the `lake exe vbp build` manifest (the blueprint render result). |
| `extract.json` | Output: `probe-leanblueprint/extract` envelope (enriched atoms). |
| `extract.summary.json` | Output: `probe-leanblueprint/summary` sidecar (two-axis counts). |
| `blueprint-stats.txt` | Output: `scripts/blueprint_stats.py` human report. |

## probe-leanblueprint result

```
Headline: 0/5 theorems machine-confirmed fully proved (0.0%)
  (blueprint claims 1/5; 1 not backed by probe-lean's verification status)
Blueprint nodes: 9   (bound 4 · planned-only 3 · decl-missing 2 · partial-missing 0 · mismatches 0)

Statement axis        all  def  thm        Proof axis         all  def  thm
  formalized            6    1    5          fully-proved        1    0    1
  ready                 2    2    0          proved              4    1    3
  blocked               1    1    0          ready               2    2    0
  none                  0    0    0          none                2    1    1

By chapter            nodes  stmt✓  thm✓/thm
  Addition                4      2       0/2
  Collatz                 2      2       0/1
  Multiplication          3      2       1/2
```

## Comparison with the blueprint's own rendering

The Verso build renders a **Blueprint Summary** page
(`_out/site/html-multi/Blueprint-Summary/`). Its counts and
`probe-leanblueprint`'s reconcile node-for-node:

| Concept | Verso Blueprint Summary | probe-leanblueprint |
|---------|-------------------------|---------------------|
| Total nodes | 9 | 9 |
| Theorems, fully closed / total | "Fully closed 1"; theorems `completed: 1` of 5 | blueprint-claimed **1/5**; proof `fully-proved` = 1 (thm) |
| Locally formalized, deps not all closed | theorems `deps incomplete: 3` | proof `proved` = 3 (thm) |
| Proof contains `sorry` | `sorries: 1` (collatz_conjecture) | proof `incomplete` → mapped to `none` (1 thm) |
| Statement formalized (defs + thms) | 6 nodes carry formalized statements | statement `formalized` = 6 |
| Informal-only / ready | "Ready now 3"; `Informal-only entries 3` | statement `ready` = 2, `blocked` = 1 |

The blueprint's claim (**1/5 theorems fully proved**) matches the rendered
summary ("Fully closed 1" of 5). probe-leanblueprint's *machine-confirmed*
headline is **0/5**, because the one fully-proved theorem is `multiplication_assoc`,
bound to the stdlib decl `Nat.mul_assoc` — which is not in this project's own
probe-lean extract, so the machine cannot confirm it here. Both numbers are
emitted; the machine-confirmed one is the honest verified-progress figure (P26).

### Decl binding

The adapter binds decls from both `codeData.external.decls[].canonical` (a
reference to an existing decl) and `codeData.inline.code.definedDefs/
definedTheorems[].name` (a decl defined inline in the blueprint text). For this
template that binds **4** project atoms — `collatz_conjecture`, `nat_add_zero_right`,
`multiplication_one_right`, and the collatz defs — with `decl-missing 2` for the
two stdlib externals (`Nat.add_assoc`, `Nat.mul_assoc`) that this project does
not itself define.
