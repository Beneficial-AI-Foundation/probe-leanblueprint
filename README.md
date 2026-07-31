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
| **A `probe-lean`-analyzable project** | The atom base comes from `probe-lean extract` (run automatically, or supplied via `--lean`). `probe-lean` itself is auto-installed, version-matched to the project's `lean-toolchain`, if not already present (see [`docs/USAGE.md`](docs/USAGE.md#probe-lean-installation)). The project must meet [probe-lean's requirements](https://github.com/Beneficial-AI-Foundation/probe-lean#supported-projects) — a buildable Lean library, compatible toolchain, etc. probe-lean ≥ v0.10.0 emits Schema 3.0 (consumed directly); older releases (≤ v0.9.6) and cached extracts emit 2.x and are auto-migrated, so no re-extraction is needed. |
| **A blueprint in a supported ecosystem** | Either a **Verso** blueprint (`versoBlueprint` declared in the root or `docs/` lakefile) or a **Massot** `leanblueprint` (`blueprint/src/web.tex`). Auto-detected; fails with a clear error when neither is present. |
| **Verso v4.30 / v4.31** | Reads the `graphs[].nodes[]` schema, recognized via `vbpInternalSchemaVersion` **2** (v4.30) and **3** (v4.31). Another generation still parses if it carries a graph, but warns that the version marker is unrecognized/absent. Pre-graph renderers (≤ v4.28) emit only a flattened preview manifest with no graph to score. |
| **A formal blueprint graph bound to decls** | Useful output requires blueprint *entries* — Verso `definition`/`theorem` nodes, or Massot `\begin{theorem}` + `\lean{...}`/`\leanok`/`\uses{...}` — that bind real Lean declarations. Progress (statement/proof status, dependency edges) lives on these nodes. |

**Checking a Verso project yourself:** run `lake exe vbp build` in the blueprint root (wherever `versoBlueprint` is declared — the project root, or its `docs/` subproject). If it succeeds and produces `_out/site/blueprint-manifest.json`, the project qualifies; probe-leanblueprint runs this same command automatically in zero-config mode.

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

## Quick start

Install, then point it at a Lean project — no flags, no pre-built inputs:

```bash
cargo install --path .
probe-leanblueprint extract path/to/lean-project
```

From the project path alone the tool auto-detects the ecosystem, produces the
atom base (via `probe-lean extract`), renders the blueprint, and writes the
enriched extract plus a two-axis progress summary under
`<project>/.verilib/probes/`.

> **Trust note.** Zero-config executes the target project's own build code
> (`probe-lean extract`, and `lake exe vbp build` via `sh -c`), and may also
> install `probe-lean` itself — downloading a prebuilt release from GitHub or
> building it from source — if no version matching the project's
> `lean-toolchain` is already cached under `~/.local/bin`. Only run it
> against projects you trust; to ingest an untrusted repo, render/extract it in
> your own sandbox and pass the results in. See [`docs/USAGE.md`](docs/USAGE.md).

Full install (incl. the Massot Python path), manual flags, output formats, the
`blueprint_stats.py` reporter, and development commands are in
[`docs/USAGE.md`](docs/USAGE.md). Output schemas are in
[`docs/SCHEMA.md`](docs/SCHEMA.md). How the extract pipeline and the
atom↔blueprint join work internally is in
[`docs/architecture.md`](docs/architecture.md).
