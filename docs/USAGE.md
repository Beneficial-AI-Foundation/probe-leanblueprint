# Usage Guide

Detailed install, flags, output formats, stats reporting, and development for
`probe-leanblueprint`. For an overview and the zero-config quick start, see the
[README](../README.md).

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

## Zero-config: just a Lean project

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

> **Trust note.** Steps 2–3 execute the target project's own build code
> (`probe-lean extract`, and `lake exe vbp build` via `sh -c`). Only run
> zero-config against projects you trust. To ingest an untrusted or third-party
> repository, render/extract it under your own sandbox first and pass the
> results in: `--lean <atoms.json>` for the atom base, and `--no-render` with
> `--verso-manifest <manifest>` (or `--blueprint-src` for Massot) for the
> blueprint side, so this tool runs no project code.

## Manual flags

Everything auto-detected can be overridden:

```bash
# Skip rendering: point at an already-rendered manifest (file or dir to search)
probe-leanblueprint extract path/to/project \
    --lean lean_atoms.json \
    --verso-manifest path/to/blueprint-manifest.json

# Custom Verso render command (run via `sh -c` in the blueprint root — the project
# root, or its docs/ subproject when versoBlueprint is declared there). It must write
# its blueprint-manifest.json under _out/site; otherwise pass --verso-manifest.
probe-leanblueprint extract path/to/project --verso-render-cmd "scripts/render-docs-site.sh"
probe-leanblueprint extract path/to/project --no-render   # require a pre-rendered manifest

# Massot leanblueprint project
probe-leanblueprint extract path/to/project \
    --adapter massot --blueprint-src blueprint/src/web.tex \
    --lean lean_atoms.json
```

## Outputs

Default under `<project>/.verilib/probes/`:

- `leanblueprint_<pkg>[_<version>].json` — enriched atoms (`probe-leanblueprint/extract`)
- `leanblueprint_<pkg>[_<version>]_summary.json` — two-axis progress counts, incl. a per-chapter breakdown (`probe-leanblueprint/summary`)

The extract envelope is an atoms-category Schema 3.0 file, so `probe merge` /
`probe project` accept it and preserve the `blueprint-*` extension fields.

Both output formats — the enriched-atom envelope and the summary sidecar,
including every `blueprint-*` field — are specified in [`SCHEMA.md`](SCHEMA.md).

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
