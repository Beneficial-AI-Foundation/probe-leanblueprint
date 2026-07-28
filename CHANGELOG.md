# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Schema **2.1** (additive over 2.0). Extract gains optional `blueprint-source-statement-status` / `blueprint-source-proof-status`, preserving a raw Verso status when the canonical enum is lossy (`mathlib` → `formalized`, `incomplete` → `none`). Summary gains `theorems-fully-proved-machine-confirmed` / `fraction-machine-confirmed`.
- Verso adapter reads and validates `vbpInternalSchemaVersion` (2 = v4.30, 3 = v4.31), warning on an unknown generation and distinguishing a previews-only blueprint (0 graph nodes, N previews) from a wrong/drifted file (0 nodes, 0 previews).
- `blueprint-shadow` extension field: marks the synthetic atom preserved for a blueprint node that loses a same-decl collision, so the extract stays node-complete and `blueprint_stats.py` is a faithful cross-check of the summary sidecar (it counts a shadow node as bound).
- `--source-package` / `--source-version` overrides to set the atom base's identity directly; supplying both also bypasses the ambiguous-provenance check for a spine with multiple probe-lean inputs.

### Changed
- Verso adapter binds declarations authored inline in the blueprint text (`codeData.inline.code.definedDefs` / `definedTheorems[].name`) in addition to `codeData.external.decls[].canonical`; on the canonical v4.31 template this lifts bound nodes from 0 to 4.
- Chapter is derived from each node's `href` (authoritative for single-shared-manifest renders), falling back to the manifest directory only when a node carries no href.
- Output writes are atomic (temp file + rename) and `--output` / `--summary-output` are validated to differ from each other and from any input file.
- Enrichment reworked around an ownership pass. `blueprint-statement-uses` / `blueprint-proof-uses` now always resolve to a code-name that exists in the atom map (previously a `uses` edge targeting a decl-missing node resolved to an absent `probe:<decl>` key). Same-decl collision losers are preserved as `blueprint-shadow` synthetic atoms (keep-last still wins the real atom) so every model node appears in the extract, and the loser keeps its `blueprint-status-mismatch` / `blueprint-missing-decls` signal.
- Re-enrichment is now idempotent across model changes: `enrich` scrubs prior synthetic blueprint atoms and stale `blueprint-*` fields before joining, so deleting or renaming a node no longer leaks a stale synthetic atom or label. Running on a merged spine that carries old blueprint atoms is supported (they are scrubbed); re-ingesting probe-leanblueprint's own `probe-leanblueprint/extract` as `--lean` is now rejected with a clear error.
- The Massot adapter de-duplicates node labels via the shared merge policy (matching Verso) instead of trusting the Python emitter's dedup.
- Adapter auto-detection checks the lakefile `versoBlueprint` signal before `blueprint/src/web.tex`, so a Verso project with a leftover/migrated Massot tree resolves to Verso (warns when both signals are present).
- Adapter auto-detection also scans the conventional `docs/` subproject lakefile for the `versoBlueprint` signal, so a project that declares the blueprint dependency only in `docs/lakefile.toml` (e.g. KVAC-model) is detected instead of failing. The `AdapterUndetected` error now spells out where it looked and how to override.
- `NodeKind::from_source` warns on a non-empty unrecognized kind instead of silently bucketing it as a theorem; absent / `null` / empty kinds still default to theorem.
- Depend on the `probe` hub via a pinned git dependency instead of a local path, so the crate builds standalone (as its `repository` field and `cargo install --path .` instructions imply).

### Fixed
- The summary headline reports theorems **machine-confirmed** fully proved (bound and not contradicted by probe-lean's `verification-status`), not the blueprint's own claim, so a `declared` (Massot `\leanok`) blueprint can no longer over-claim verified progress (P26). `theorems-fully-proved` is retained as the blueprint-claimed number.
- A null-kind *mention* of a node defined in another chapter no longer freezes it as a theorem in the wrong chapter with the wrong title: a known-kind (defining) copy now wins the kind/chapter/title during merge regardless of manifest order. Corrects the secure-messaging headline from 9/56 to 8/53 (three definitions were miscounted as theorems, one of them fully proved).
- `Cargo.lock` now pins the `probe` git `source`/rev; the committed lockfile had been generated behind a local `[patch]` and omitted it, so clean builds re-resolved to a different revision.
- Verso adapter accepts the ≥ v4.31 blueprint-manifest status vocabulary instead of failing with a schema-drift error: `mathlib` (statement already upstream in Mathlib) maps to `formalized` on the statement axis, and `incomplete` (Lean proof present but containing `sorry`) maps to `none` on the proof axis (an incomplete proof is not a complete one, so it never counts as proved and keeps the `blueprint-status-mismatch` check honest). The raw status is preserved in `blueprint-source-*-status`.
- `select_source` no longer misclassifies probe-leanblueprint's own `probe-leanblueprint/*` provenance as a probe-lean input (the `probe-lean/` prefix is now matched with its delimiter). The ambiguous-provenance error points at `--source-package` / `--source-version` rather than the ineffective `-o` / `--summary-output`.

## [0.2.0] - 2026-07-21

### Added
- `blueprint-chapter` extension field and a `by-chapter` breakdown in the summary sidecar (per-chapter node count, two-axis histograms, theorems-fully-proved/total). Chapter is derived robustly from the Verso manifest path.
- `blueprint-kind` extension field so consumers can classify bound atoms (whose atom `kind` is the Lean kind) as blueprint definition vs theorem.
- `blueprint-missing-decls` extension field: for a bound node, the subset of `\lean{...}` declarations absent from the probe-lean atom base (partial miss), with a matching `partial-missing` summary total. Distinct from `blueprint-decl-missing`, which stays reserved for the all-absent synthetic-node case.
- `collisions` and `partial-missing` counters in the summary `totals`; enrichment now warns when one Lean declaration is bound by multiple blueprint nodes (keep-last) and when a synthetic key is produced twice.
- `scripts/blueprint_stats.py`: render a two-axis + per-chapter progress report from an `extract.json` (pure stdlib; also `--json`); now surfaces partial-missing declarations and enforces the exact `probe-leanblueprint/extract` schema.
- Typed library errors (`BlueprintError`, `thiserror`); the CLI keeps `anyhow` at its boundary.
- Full-project e2e fixture and test: all 9 secure-messaging chapter manifests (freshly rendered at commit 6a4fce0, trimmed to adapter-relevant fields) joined onto the same-commit real probe-lean atom base, pinning the authoritative numbers (111 nodes, 33 bound, 0 decl-missing, 0 mismatch, 9/56 theorems fully proved). Verified consistent with the deployed per-chapter Blueprint-Summary pages (e.g. AEAD: 14 nodes, 5 theorems, 3 fully proved).

### Changed
- The Massot emitter (`blueprint_emit.py`) is now embedded in the binary via `include_str!` and materialized to a temp file at runtime, so a `cargo install`ed executable is self-contained (previously relied on a compile-time `CARGO_MANIFEST_DIR` path). `--emitter` and a copy shipped next to the executable still take precedence.
- Re-enrichment is now idempotent: all `blueprint-*` keys are rewritten each run so a stale `blueprint-status-mismatch`/`blueprint-decl-missing` from a prior pass can no longer leak.
- `merge_from` now set-unions `lean_decls`/`statement-uses`/`proof-uses` across duplicate-label nodes (previously dropped later bindings) while keeping status-max and first-wins for descriptive fields; Verso manifests are also de-duplicated within a single manifest.
- Status mismatches are de-duplicated per blueprint node, matching `blueprint_stats.py`'s per-label counting.

### Fixed
- Verso adapter now errors on an unknown statement/proof status string (schema drift) instead of silently bucketing it to the worst state and under-counting progress.
- Auto-detection now fails with a clear message when no adapter signal is present, instead of defaulting to Verso and dying later with a confusing "no manifest" error.
- Default output filenames sanitize `/`, `\`, and `..` in the package/version, and directory-creation failures are propagated instead of ignored.
- Provenance `Source` selection prefers the `probe-lean` input rather than the first `inputs` entry.

## [0.1.0] - 2026-07-21

### Added
- Initial `probe-leanblueprint extract` command: enrich `probe-lean/extract` atoms with Lean blueprint two-axis (statement/proof) progress metadata.
- Verso Blueprint adapter: parse `blueprint-manifest.json` (graph nodes + `codeData.external.decls[].canonical` bindings) into a normalized `BlueprintModel`.
- Massot `leanblueprint` adapter: bundled headless plasTeX emitter (`scripts/blueprint_emit.py`) reusing leanblueprint's own parser; shell out and normalize its JSON.
- Enrichment core: join blueprint nodes to atoms by `probe:<canonical>`, synthesize planned atoms for nodes with no Lean binding, flag `blueprint-status-mismatch` and `blueprint-decl-missing`, reuse `probe::propagate`.
- Outputs: `probe-leanblueprint/extract` Schema 2.0 envelope + a `probe-leanblueprint/summary` sidecar aggregating two-axis progress counts over the blueprint nodes.
