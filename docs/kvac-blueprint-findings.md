# Why `probe-leanblueprint` "failed" (then found nothing) on KVAC-model

A note for reviewers of the Verso-adapter PR, prompted by:

> Just to check — the tool only looks for the `versoBlueprint` package in the
> root `lakefile.toml`, right? I tried running it on KVAC-model but it failed.
> Then I remembered KVAC-model has a separate `lakefile.toml` under `docs/`
> which is only meant for `versoBlueprint`.

Short answer: you were right about the root cause of the *failure*, that part
is now fixed. But there are **two independent reasons** KVAC gives you nothing,
and the second one is not about the toolchain at all.

## Layer 1 — adapter auto-detection was root-only (fixed)

`detect_adapter` (auto mode) scanned only the **project-root** lakefile for the
`versoBlueprint` signal:

- KVAC's root `lakefile.toml` has no verso signal (and there's no root
  `lakefile.lean`);
- the `versoBlueprint` dependency is declared only in `docs/lakefile.toml`;
- there's no `blueprint/src/web.tex` (no Massot signal either).

So detection resolved to `(has_verso=false, has_web_tex=false)` →
`AdapterUndetected`. That's the failure you hit.

**Fix (in this PR):** auto-detection now also scans the conventional `docs/`
subproject lakefile, and the `AdapterUndetected` error spells out where it
looked and how to override (`--adapter`, or point the project path at the
subproject). Workarounds without the fix: `probe-leanblueprint extract docs
--adapter verso …`, or pass `--verso-manifest …` explicitly.

## Layer 2 — KVAC's pinned versoBlueprint was too old (version cliff)

KVAC's `docs/lakefile.toml` pinned **versoBlueprint v4.28.0**, whose renderer
emits only the flattened `blueprint-preview-manifest.json` (a `previews` array,
no `graphs`). The Verso adapter consumes `graphs[].nodes[]`, so v4.28.0 yields
nothing regardless of detection. The `graphs` schema first appears in
**v4.29.0+**.

This is what we hit when we forced the adapter earlier and "got nothing
interesting." It looked like the whole story. It isn't.

## Layer 3 — KVAC's blueprint has no graph, only informal previews (the real reason)

We ported KVAC end-to-end to **v4.30.0** (Lean + Mathlib + VCVio +
versoBlueprint, on an experimental branch) so the renderer *can* emit the
`graphs` schema, built it, rendered the blueprint, and ran the tool against the
real manifest:

```
Blueprint nodes: 0 (0 bound, 0 planned-only, 0 decl-missing, 0 partial-missing, 0 collisions); mismatches: 0
```

The rendered `blueprint-manifest.json` (schema `vbpInternalSchemaVersion: 2`)
contains:

- `graphs: []`
- `previews: 4` — informal `LeanCodePreview` blocks previewing two Lean decls
  (`KVAC.Core.PrimeOrderGroup`, `KVAC.Core.SampleableGroup`).

KVAC's blueprint document is authored **entirely with informal previews**. It
never declares a formal blueprint graph — i.e. the nodes carrying
`statementStatus` / `proofStatus` that live under `graphs[].nodes[]`. The
adapter builds its model *only* from `graphs[].nodes[]` (previews merely supply
decl names to graph nodes that reference a `previewKey`), so it binds **0 nodes
regardless of the toolchain version**.

### Takeaway

- The detection failure is fixed.
- The version cliff (v4.28 → v4.29+) is real but secondary.
- To get meaningful `probe-leanblueprint` output on KVAC, its `docs/` content
  has to be rewritten to use actual blueprint **graph** nodes, not just
  informal `LeanCodePreview` blocks. That's a KVAC authoring change, unrelated
  to this tool or the toolchain.

For a project authored with a real blueprint graph, see the working
end-to-end captures under `examples/` (Sphere Packing, Carleson, FLT,
Noperthedron).
