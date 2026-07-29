# probe-leanblueprint

Enrich [`probe-lean`](https://github.com/Beneficial-AI-Foundation/probe-lean)
Schema 3.0 atoms with Lean **blueprint** progress metadata, so Lean projects get
meaningful verification-progress statistics instead of a bare theorem count.

A blueprint captures, per declaration, a **two-axis status**:

- **statement** — is the *statement* formalized in Lean? (`none` → `blocked` → `ready` → `formalized`)
- **proof** — is the *proof* complete and sorry-free? (`none` → `ready` → `proved` → `fully-proved`)

`probe-leanblueprint` reads that status from either blueprint ecosystem, joins it
onto probe-lean's code call graph, and re-emits a Schema 3.0 envelope plus a
two-axis progress summary.

## Supported blueprint ecosystems

| Ecosystem | Source | Notes |
|-----------|--------|-------|
| **Verso Blueprint** (`versoBlueprint`) | `blueprint-manifest.json` | Lean-native; status is code-derived |
| **Patrick Massot `leanblueprint`** | `blueprint/src/web.tex` | LaTeX/plasTeX; status is human-declared (`\leanok`) |

`probe-lean`'s machine `verification-status` stays authoritative on the proof
axis; the blueprint's claim is additive, and a `blueprint-status-mismatch` flag
fires when the blueprint over-claims a proof (see the KB, property P26).

## Supported Projects

probe-leanblueprint enriches a `probe-lean` atom base, so it runs on any project
`probe-lean` can analyze that **also ships a blueprint with a formal graph bound
to Lean declarations**.

| Requirement | Detail |
|-------------|--------|
| **A `probe-lean`-analyzable project** | The atom base comes from `probe-lean extract` (run automatically, or supplied via `--lean`). The project must meet [probe-lean's requirements](https://github.com/Beneficial-AI-Foundation/probe-lean#supported-projects) — a buildable Lean library, compatible toolchain, etc. probe-lean ≥ v0.10.0 emits Schema 3.0 (consumed directly); older releases (≤ v0.9.6) and cached extracts emit 2.x and are auto-migrated, so no re-extraction is needed. |
| **A blueprint in a supported ecosystem** | Either a **Verso** blueprint (`versoBlueprint` declared in the root or `docs/` lakefile) or a **Massot** `leanblueprint` (`blueprint/src/web.tex`). Auto-detected; fails with a clear error when neither is present. |
| **Verso v4.30 / v4.31** | Reads the `graphs[].nodes[]` schema, recognized via `vbpInternalSchemaVersion` **2** (v4.30) and **3** (v4.31). Another generation still parses if it carries a graph, but warns that the version marker is unrecognized/absent. Pre-graph renderers (≤ v4.28) emit only a flattened preview manifest with no graph to score. |
| **A formal blueprint graph bound to decls** | Useful output requires blueprint *entries* — Verso `definition`/`theorem` nodes, or Massot `\begin{theorem}` + `\lean{...}`/`\leanok`/`\uses{...}` — that bind real Lean declarations. Progress (statement/proof status, dependency edges) lives on these nodes. |

Known-working projects, captured end-to-end (manifest + `probe-lean` atoms →
enriched extract + summary), live under [`examples/`](examples/): Sphere Packing,
Carleson, FLT, Noperthedron, and the verso-blueprint `project_template`.

### What won't work

- **Projects with no blueprint** — no `versoBlueprint` lakefile signal and no
  `blueprint/src/web.tex`. Detection fails loudly (`AdapterUndetected`) rather
  than guessing.
- **Verso blueprints authored only with informal previews** — a document built
  entirely from `LeanCodePreview` blocks (no formal `definition`/`theorem`
  entries) renders a manifest with an empty `graphs` array. The run succeeds but
  binds **0 nodes** (the tool warns "0 graph nodes but N previews"); there is
  nothing to score until the blueprint declares graph entries bound to decls.
- **versoBlueprint < v4.29** — the pre-`graphs` renderer emits only a
  `blueprint-preview-manifest.json`, which yields a zero-node model.
- **Projects `probe-lean` cannot analyze** — no atom base means nothing to
  enrich (see [probe-lean's constraints](https://github.com/Beneficial-AI-Foundation/probe-lean#supported-projects)).

## Install

```bash
cargo install --path .
```

For the **Massot** path, install the Python emitter's dependencies (the Verso
path is pure Rust and needs no Python):

```bash
# needs graphviz + headers for pygraphviz
sudo apt-get install graphviz libgraphviz-dev
pip install -r requirements.txt   # plasTeX, plastexdepgraph, leanblueprint
```

## Usage

### Zero-config: just a Lean project

The intended entry point (and the one automated consumers use) needs **only a
Lean project path** — no flags, no pre-built inputs:

```bash
probe-leanblueprint extract path/to/lean-project
```

From that alone the tool:

1. **auto-detects the ecosystem** from the project — a `versoBlueprint`
   dependency in `lakefile.toml`/`lakefile.lean` → Verso; a
   `blueprint/src/web.tex` tree → Massot (fails loudly if neither is present);
2. **produces the atom base** by running `probe-lean extract` (unless `--lean`
   is given);
3. **produces the blueprint data itself** — for Verso it runs `lake exe vbp
   build` when no `blueprint-manifest.json` exists yet (that render entry point
   ships with the `versoBlueprint` dependency); for Massot it runs the bundled
   plasTeX emitter (embedded in the binary). No manual render step is required.

See [the tool KB](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/tools/probe-leanblueprint.md#zero-config-contract-only-a-lean-project-in)
for the full contract.

### Manual flags

Everything auto-detected can be overridden:

```bash
# Skip rendering: point at an already-rendered manifest (file or dir to search)
probe-leanblueprint extract path/to/project \
    --lean lean_atoms.json \
    --verso-manifest path/to/blueprint-manifest.json

# Custom Verso render command (run via `sh -c` in the project dir), or opt out
probe-leanblueprint extract path/to/project --verso-render-cmd "scripts/render-docs-site.sh"
probe-leanblueprint extract path/to/project --no-render   # require a pre-rendered manifest

# Massot leanblueprint project
probe-leanblueprint extract path/to/project \
    --adapter massot --blueprint-src blueprint/src/web.tex \
    --lean lean_atoms.json
```

Outputs (default under `<project>/.verilib/probes/`):

- `leanblueprint_<pkg>[_<version>].json` — enriched atoms (`probe-leanblueprint/extract`)
- `leanblueprint_<pkg>[_<version>]_summary.json` — two-axis progress counts, incl. a per-chapter breakdown (`probe-leanblueprint/summary`)

The extract envelope is an atoms-category Schema 3.0 file, so `probe merge` /
`probe project` accept it and preserve the `blueprint-*` extension fields.

Both output formats — the enriched-atom envelope and the summary sidecar,
including every `blueprint-*` field — are specified in [`docs/SCHEMA.md`](docs/SCHEMA.md).

## Displaying stats

`scripts/blueprint_stats.py` renders a readable progress report straight from an
`extract.json` — headline, statement/proof tables, a per-chapter breakdown, and
any status mismatches / missing declarations. It recomputes everything from the
`blueprint-*` extension fields (pure stdlib Python, no blueprint deps), so it
also serves as an independent cross-check of the summary sidecar.

```bash
python3 scripts/blueprint_stats.py path/to/leanblueprint_<pkg>.json
python3 scripts/blueprint_stats.py path/to/leanblueprint_<pkg>.json --json  # machine-readable
```

Example (secure-messaging):

```
Headline: 8/53 theorems probe-lean-confirmed fully proved (15.1%)
Blueprint nodes: 111   (bound 33 · planned-only 78 · decl-missing 0 · partial-missing 0 · mismatches 0)

By chapter                            nodes  stmt✓  thm✓/thm
  Authenticated-Encryption-with-A...     14     12       3/5
  Continuous-Key-Agreement               15     12       4/6
  Erasure-Codes                           4      2       0/1
  ...
```

(58 definitions + 53 theorems = 111 nodes. The headline reports theorems
*probe-lean-confirmed* fully proved: bound to a present atom and not contradicted by
probe-lean. This is a "the machine hasn't refuted this" bar, not "affirmatively
verified" — a bound theorem with no `verification-status`, a `trusted` one, or
one only locally `verified` still counts. It is *not* the same as the blueprint's
own claim (`theorems-fully-proved`), even for a code-derived Verso blueprint: a
fully-proved node that is decl-missing or unbound inflates the claim but is never
confirmed. For a `declared` Massot blueprint it additionally drops any `\leanok`
the machine contradicts.

A decl-missing node is split further: one whose binding is an *out-of-workspace*
decl the Verso renderer reports present and proved (a dependency — commonly
Mathlib/stdlib, but only out-of-workspace is actually checked) is proved
elsewhere, just not in this project's extract — a genuine gap it is not.
Those are reported separately as `+K upstream-proved` on the headline (and
`decl-missing-upstream-proved` in the totals) so "proved upstream" is never
conflated with "not found". The blueprint-project-template reads `0/5
probe-lean-confirmed (+1 upstream-proved)`: its one fully-proved theorem binds
`Nat.mul_assoc`, proved in stdlib, absent from the template's own atoms.)

## How it works

See the ecosystem knowledge base:

- [`kb/tools/probe-leanblueprint.md`](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/tools/probe-leanblueprint.md) — tool spec (pipeline, join rules, extension fields)
- [`kb/decisions/004-probe-leanblueprint.md`](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/decisions/004-probe-leanblueprint.md) — design rationale (ADR-004)
- [`kb/engineering/properties.md`](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/engineering/properties.md) — P26 (additive blueprint status), P3 (stub detection)

## Development

```bash
cargo test                       # unit + integration tests (no Python needed; uses fixtures)
cargo test -- --ignored          # also runs the live plasTeX emitter test
cargo clippy --all-targets -- -D warnings
```

The live Massot emitter test needs a Python with plasTeX + leanblueprint:

```bash
PROBE_LEANBLUEPRINT_PYTHON=/path/to/venv/bin/python cargo test -- --ignored
```
