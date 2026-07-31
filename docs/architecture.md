# Architecture

How `probe-leanblueprint` works internally: the extract pipeline, the
atom↔blueprint join, and the source-file map. This is the normative home for the
tool's **mechanics**.

- Output **semantics** (status axes, node classification, every `blueprint-*`
  field, the two envelopes) are normative in [`SCHEMA.md`](SCHEMA.md) — this
  document references those definitions rather than restating them.
- User-facing overview, install, and flags are in the [README](../README.md) and
  [`USAGE.md`](USAGE.md).

`probe-leanblueprint` is an **enricher**, analogous to `probe-aeneas`: it
consumes another probe's output (`probe-lean/extract` atoms) as its atom spine
and re-emits a Schema 3.0 envelope with extra fields. It does not re-implement
blueprint parsing, does not touch probe-lean (which stays blueprint-unaware), and
does not override the machine `verification-status` — the blueprint proof axis is
additive (see [`SCHEMA.md` → Machine reconciliation](SCHEMA.md#machine-reconciliation-p26)).

## Extract pipeline

The `extract` command (`src/main.rs` → `src/enrich.rs`):

```
project → (probe-lean extract | --lean) → atom base
        → adapter (Verso manifest | Massot plasTeX) → BlueprintModel
        → join by probe:<canonical> → enrich atoms + synthesize planned atoms
        → propagate::enrich_verification_status (idempotent)
        → probe-leanblueprint/extract envelope + probe-leanblueprint/summary sidecar
```

1. **Resolve adapter** — explicit `--adapter`, else auto-detect: a
   `versoBlueprint` dependency in the lakefile (or `--verso-manifest`) → Verso;
   `blueprint/src/web.tex` (or `--blueprint-src`) → Massot. Neither present →
   `AdapterUndetected`.
2. **Load atom base** — `--lean <probe-lean.json>` if given, else run `probe-lean
   extract <project>`. `probe-lean` is auto-installed, version-matched to the
   project's `lean-toolchain`, when absent (see
   [`USAGE.md` → probe-lean installation](USAGE.md#probe-lean-installation)). A
   `probe-lean/extract` still on 2.x is re-stamped to 3.0 in a temp copy before
   loading (the original is untouched); passing this tool's own
   `probe-leanblueprint/extract` back in is rejected (self-ingestion).
3. **Build the blueprint model** — the Verso adapter parses
   `blueprint-manifest.json`, rendering it first (`lake exe vbp build`, or
   `--verso-render-cmd`) when none exists under `_out/site`; `--no-render`
   requires a pre-rendered manifest. The Massot adapter shells out to the bundled
   `scripts/blueprint_emit.py`, which is **embedded into the binary**
   (`include_str!`) and materialized to a temp file at runtime, so a `cargo
   install`ed executable is self-contained (an explicit `--emitter` or a copy
   shipped next to the executable takes precedence).
4. **Join + enrich** — match blueprint nodes to atoms by `probe:` + Lean
   declaration name; attach `blueprint-*` fields; synthesize planned /
   decl-missing atoms; compute `blueprint-status-mismatch`. See
   [The join](#the-join).
5. **Propagate** — reuse `probe::commands::propagate::enrich_verification_status`
   (`src/main.rs`; idempotent — the machine `verification-status` stays
   authoritative).
6. **Emit** — the enriched atom envelope and the summary sidecar (an aggregate
   over the blueprint nodes).

### Single-build guarantee

Lake builds are incremental and the code libraries are shared between the code
target and the Verso docs/blueprint target, so the total cost is **one full
compile**: rendering the Verso docs (which writes `blueprint-manifest.json`)
compiles the libs, and the subsequent `probe-lean extract` is an incremental
no-op on the already-compiled libs. The Massot/LaTeX path needs no Lean docs
build at all — plasTeX only parses LaTeX.

## The join

Both ecosystems bind a blueprint node to Lean declarations by **user-facing
fully-qualified name**: Massot via `\lean{Foo.bar}`, Verso via
`codeData.external.decls[].canonical`. probe-lean keys atoms as `probe:` + that
same user-facing name (`probeRef`), so the join is `probe:<canonical>`.

The node-classification buckets themselves (bound / planned-only / decl-missing /
partial-missing / collision-shadow, and the upstream-proved split) are defined in
[`SCHEMA.md` → Node classification](SCHEMA.md#node-classification). The
**algorithm** that produces them (`src/enrich.rs`):

- **Ownership (pass A, keep-last).** Compute, per present atom, the *last*
  blueprint node that binds it. Each re-binding of an already-claimed atom is a
  collision (counted in the summary; a warning is logged).
- **Primary key (pass B).** Resolve every node to the record that will hold it:
  the first present atom it owns, else its synthetic `probe:blueprint:<label>`
  key. This makes the extract **node-complete** — every model node leaves exactly
  one label-bearing record, so `uses` edges always resolve to a real atom key and
  `scripts/blueprint_stats.py` recomputes the sidecar counts exactly.
- **Node binds multiple decls** — attach the node to every present atom it owns.
- **Same-decl collision** — the later node wins the real atom (keep-last); the
  **losing** node is preserved as a synthetic `blueprint-shadow: true` atom
  (carrying its full status and any mismatch / missing-decls) so it is not
  dropped. A shadow still counts as bound (`with-lean-decl`).
- **Decl-missing authority** — probe-lean atom membership is the **sole**
  authority on whether a bound declaration is present. Verso's own per-decl
  `present` / node `missingExternalDecl` hints are intentionally **not** consumed:
  they coincide with atom membership on real data, and atom membership is the
  tool's premise that probe-lean is the code spine. (The one exception is
  `provenance.outWorkspace`, used to tell a dependency-proved decl from a genuine
  gap — see the upstream-proved split in `SCHEMA.md`.)
- **Planned-only node** (no Lean binding) — synthesize a `probe:blueprint:<label>`
  atom with `language: "blueprint"`, `kind: "blueprint-<def|theorem>"`, and a
  non-empty `code-path` marker (`"blueprint"`) so structural stub detection does
  not misclassify it.
- **`uses` edges stay extension-only** (`blueprint-statement-uses` /
  `blueprint-proof-uses`) — the informal roadmap graph is never merged into an
  atom's `dependencies` (the code call graph).

## Adapters

| Adapter | Source | How it is read | Status authority |
|---------|--------|----------------|------------------|
| Verso (`src/adapters/verso.rs`) | `blueprint-manifest.json` | Parse the JSON directly | code-derived |
| Massot (`src/adapters/massot.rs`) | `blueprint/src/web.tex` | Bundled headless plasTeX emitter reusing leanblueprint's own parser | human-declared |

Both normalize their native vocabulary into the canonical two-axis status enums
in `src/model.rs`; the per-adapter mapping tables are in
[`SCHEMA.md` → Source-status mapping](SCHEMA.md#source-status-mapping).

## Reuse of the probe hub crate

probe-leanblueprint depends on the `probe` hub crate for shared types
(`Atom`, `AtomEnvelope`, `Source`, `Tool`, `CodeText`, `load_atom_file`) and for
`probe::commands::propagate::enrich_verification_status` (reused, idempotent). The
`probe-leanblueprint/extract` envelope is an Atoms-category Schema 3.0 file, so
`probe merge` / `project` accept it and preserve the `blueprint-*` extensions. The
hub design context lives in the ecosystem KB
([ADR-004](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/decisions/004-probe-leanblueprint.md)).

## Key source files

| File | Purpose |
|------|---------|
| `src/main.rs` | CLI, adapter auto-detection, orchestration, output, probe-lean 2.x→3.0 re-stamping |
| `src/setup.rs` | probe-lean auto-install: cache lookup, prebuilt-release download, from-source fallback |
| `src/model.rs` | Normalized `BlueprintModel` / `BlueprintNode`, canonical status enums, extension field set |
| `src/adapters/verso.rs` | Verso `blueprint-manifest.json` → `BlueprintModel` |
| `src/adapters/massot.rs` | Shell out to the plasTeX emitter → `BlueprintModel` |
| `src/enrich.rs` | Join (ownership, collisions), synthesis, mismatch, summary computation |
| `src/emit.rs` | Envelope + summary sidecar construction (`SCHEMA_VERSION`) |
| `src/emitter.rs` | Embeds `blueprint_emit.py` (`include_str!`) and materializes it at runtime |
| `src/error.rs` | Error types (e.g. `AdapterUndetected`) |
| `scripts/blueprint_emit.py` | Bundled headless plasTeX emitter (reuses leanblueprint's parser); embedded into the binary |
| `scripts/blueprint_stats.py` | Display two-axis + per-chapter stats from an `extract.json` |
