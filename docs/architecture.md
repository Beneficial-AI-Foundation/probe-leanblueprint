# Architecture

`probe-leanblueprint` enriches a `probe-lean/extract` atom base with Lean
blueprint progress and re-emits it. It reads the blueprint from a Verso manifest
or a Massot LaTeX source, joins each blueprint node to the Lean declaration it
describes, and adds `blueprint-*` fields to the matching atoms. It never re-parses
Lean code and never changes probe-lean's machine `verification-status`; the
blueprint's proof status is additive.

This document covers the pipeline and where each part lives in the code. Output
semantics, the status axes, and every field are specified in
[`SCHEMA.md`](SCHEMA.md); install and flags are in [`USAGE.md`](USAGE.md).

## Pipeline

`extract` is the only command. Its steps, in order:

```
project → atom base (probe-lean extract, or --lean)
        → blueprint model (Verso manifest, or Massot plasTeX)
        → join and enrich, synthesizing atoms for unbound nodes
        → propagate verification status
        → extract envelope + summary sidecar
```

1. Detect the adapter. A `versoBlueprint` lakefile dependency selects Verso; a
   `blueprint/src/web.tex` tree selects Massot. Verso is checked first, so a
   project carrying both signals uses Verso and logs a warning. `--adapter`
   overrides.
2. Load the atom base. With `--lean` it reads a `probe-lean/extract` file;
   otherwise it runs `probe-lean extract`, first installing a version-matched
   `probe-lean` if none is cached (see
   [`USAGE.md`](USAGE.md#probe-lean-installation)).
3. Build the blueprint model. The Verso adapter parses `blueprint-manifest.json`,
   rendering it with `lake exe vbp build` first if it is missing. The Massot
   adapter runs the plasTeX emitter in `scripts/blueprint_emit.py`, which is
   embedded in the binary so an installed executable needs no separate copy.
4. Join and enrich. See [The join](#the-join).
5. Propagate transitive verification status, reusing the hub's
   `enrich_verification_status`. The machine status stays authoritative.
6. Emit the enriched atoms and the summary sidecar.

The whole run costs one Lean compile. `probe-lean extract` compiles the libraries
in step 2, and the Verso render in step 3 reuses that build incrementally. The
Massot path compiles no Lean, since plasTeX only reads LaTeX.

## The join

A blueprint node names the Lean declarations it formalizes: Massot through
`\lean{Foo.bar}`, Verso through the manifest's external and inline declaration
lists. probe-lean keys each atom by the same fully-qualified name, so a node joins
to an atom when the names match.

`src/enrich.rs` does the matching. When several nodes claim the same atom the last
one wins, and the losers are kept as shadow atoms so no node is dropped. A node
whose declarations are all absent from the atom base, or that binds no declaration
at all, becomes a synthetic atom with `language: "blueprint"`. Each node is
represented exactly once for `uses` resolution, which lets
`scripts/blueprint_stats.py` recompute the summary counts independently. The node
buckets and the two-axis status this produces are defined in
[`SCHEMA.md`](SCHEMA.md#node-classification).

Two boundaries hold for both adapters. probe-lean membership alone decides whether
a declaration is present; the only exception is Verso's upstream-proved split,
where a decl absent locally counts as proved elsewhere when the renderer marks it
out-of-workspace and proved (`is_upstream_proved` in `src/adapters/verso.rs`). And
a blueprint's `uses` graph stays in `blueprint-*` fields; it is never folded into
an atom's code dependencies.

## Adapters

| Adapter | Source | Read by | Status |
|---------|--------|---------|--------|
| Verso | `blueprint-manifest.json` | parsing the JSON | code-derived |
| Massot | `blueprint/src/web.tex` | the bundled plasTeX emitter, reusing leanblueprint's parser | human-declared |

Both map their native statuses into the canonical two-axis enums in
`src/model.rs`; the mapping tables are in
[`SCHEMA.md`](SCHEMA.md#source-status-mapping).

## The probe hub crate

The tool depends on the `probe` crate for the shared atom types and for
`enrich_verification_status`. Its `probe-leanblueprint/extract` output is an
atoms-category Schema 3.0 file, so `probe merge` and `probe project` accept it and
keep the `blueprint-*` fields. The design rationale is in
[ADR-004](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/decisions/004-probe-leanblueprint.md).

## Source files

| File | Purpose |
|------|---------|
| `src/main.rs` | CLI, adapter detection, orchestration |
| `src/setup.rs` | probe-lean auto-install: cache, release download, source build |
| `src/model.rs` | `BlueprintModel`, the canonical status enums, the field set |
| `src/adapters/verso.rs` | Verso manifest to `BlueprintModel` |
| `src/adapters/massot.rs` | plasTeX emitter to `BlueprintModel` |
| `src/enrich.rs` | the join, synthesis, mismatch, and summary |
| `src/emit.rs` | envelope and sidecar construction |
| `src/emitter.rs` | embeds and materializes `blueprint_emit.py` |
| `src/error.rs` | error types |
| `scripts/blueprint_emit.py` | headless plasTeX emitter, embedded in the binary |
| `scripts/blueprint_stats.py` | render two-axis stats from an extract |
