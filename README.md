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
Headline: 9/56 theorems fully proved (16.1%)
Blueprint nodes: 111   (bound 33 · planned-only 78 · decl-missing 0 · partial-missing 0 · mismatches 0)

By chapter                            nodes  stmt✓  thm✓/thm
  Authenticated-Encryption-with-A...     14     12       3/5
  Continuous-Key-Agreement               15     12       4/6
  Erasure-Codes                           4      2       0/1
  ...
```

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
