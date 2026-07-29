# probe-leanblueprint Data Schemas

Schema version: 3.0 (interchange; plus additive optional fields — see Schema Evolution)
Date: 2026-07-29

This document specifies the JSON output formats produced by `probe-leanblueprint`.
It complements the language-agnostic
[envelope-rationale.md](https://github.com/Beneficial-AI-Foundation/probe/blob/main/docs/envelope-rationale.md),
which defines the envelope wrapper; this document defines what goes **inside**
the `data` field of each output file.

**This document is normative.** It is the single source of truth for the tool's
output *semantics* — field meanings, the status vocabulary, and how nodes are
classified and scored. The Rust doc-comments and the ecosystem
[tool KB](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/tools/probe-leanblueprint.md)
are non-normative summaries that point here; if either disagrees with this
document, this document wins. See [Semantics](#semantics-normative) below for the
definitions; the per-field tables further down give the wire format.

`probe-leanblueprint` is an **enricher**: it consumes a `probe-lean/extract`
atom base and a blueprint (Verso manifest or Massot LaTeX), joins them by Lean
declaration name, and re-emits two files:

- `probe-leanblueprint/extract` — the enriched atoms (an atoms-category file,
  so `probe merge`/`project` accept it and preserve the `blueprint-*` fields).
- `probe-leanblueprint/summary` — a two-axis progress sidecar (never merged).

Both are produced by the single `extract` subcommand.

---

## Semantics (normative)

The definitions here are canonical. The wire-format tables in sections 1 and 2
reference them.

### Status axes

Two independent axes, each an ordered ladder (worst → best):

- **statement** — is the *statement* formalized in Lean?
  `none` < `blocked` < `ready` < `formalized`
- **proof** — is the *proof* complete (sorry-free)?
  `none` < `ready` < `proved` < `fully-proved`

Rung meanings:

| Value | Axis | Meaning |
|-------|------|---------|
| `none` | both | not started; no formalization |
| `blocked` | statement | prerequisites not ready; cannot yet be stated |
| `ready` | both | prerequisites done; ready to be stated / proved |
| `formalized` | statement | the statement is formalized in Lean |
| `proved` | proof | the proof is formalized and sorry-free **locally** (its own body), but not all dependencies are complete |
| `fully-proved` | proof | the proof **and all transitive dependencies** are complete |

The `proved` vs `fully-proved` split is load-bearing: `proved` = "this node's
proof compiles sorry-free on its own"; `fully-proved` = "this node *and*
everything it depends on are done". **Only `fully-proved` counts toward the
headline.**

### Source-status mapping

Each adapter maps its native vocabulary into the canonical axes. When a mapping
is *lossy* (the raw status carries information the canonical enum drops), the raw
value is preserved verbatim in `blueprint-source-statement-status` /
`blueprint-source-proof-status`.

Verso (`code-derived`):

| Raw `statementStatus` → statement | Raw `proofStatus` → proof |
|-----------------------------------|---------------------------|
| `none` → `none` | `none` → `none` |
| `ready` → `ready` | `ready` → `ready` |
| `blocked` → `blocked` | `formalized` → `proved` |
| `formalized` → `formalized` | `formalizedWithAncestors` → `fully-proved` |
| `mathlib` → `formalized` *(lossy: upstream in Mathlib)* | `incomplete` → `none` *(lossy: sorried/in-progress)* |

Massot (`declared`, from human `\leanok` etc.):

| Statement | Proof |
|-----------|-------|
| `leanok` or `mathlibok` → `formalized` | `proved` and `fully_proved` → `fully-proved` |
| `can_state` → `ready` | `proved` → `proved` |
| `notready` → `blocked` | `can_prove` → `ready` |
| else → `none` | else → `none` |

(`fully_proved` in leanblueprint marks definitions vacuously done, so Massot
gates `fully-proved` on `proved` too, to avoid over-claiming on definitions.)

### Status source

`blueprint-status-source` records how far to trust the proof axis:

- **code-derived** (Verso) — the renderer elaborated Lean; the status is a
  machine judgment.
- **declared** (Massot) — a human wrote `\leanok`; not machine-checked. A
  `declared` proof axis can over-claim, which is what machine reconciliation
  (below) guards against.

### Node classification

Every blueprint node lands in exactly one bucket, driven by whether/how it binds
a Lean decl present in the atom base:

- **bound** (`with-lean-decl`) — binds ≥1 present Lean decl. The present atom(s)
  gain the `blueprint-*` fields.
- **planned-only** — binds no Lean decl at all (roadmap only). Emitted as a
  synthetic `language: "blueprint"` atom.
- **decl-missing** — binds ≥1 Lean decl but *every* one is absent from the atom
  base. Emitted synthetic, flagged `blueprint-decl-missing`. Split further:
  - **upstream-proved** — *every* binding is an external decl the Verso renderer
    reports as **out-of-workspace** (its `provenance.outWorkspace`) **and**
    present **and** proved: proved elsewhere, absent here — not a genuine gap.
    "Out-of-workspace" is exactly what's checked (a decl in some dependency built
    on, *commonly* Mathlib/stdlib but not verified to be — could be any
    out-of-workspace package); "upstream" is shorthand for that, not a namespace
    claim. Flagged `blueprint-decl-upstream-proved`. code-derived only (Massot
    carries no per-decl provenance).
  - **genuine gap** — everything else decl-missing.
- **partial-missing** — a *bound* node where some (not all) decls are absent; the
  absent names go in `blueprint-missing-decls` on the present atom(s).
- **collision-shadow** — a node whose present decl was claimed by a later node
  (keep-last); preserved as a synthetic `blueprint-shadow` atom so the extract
  stays node-complete. Counts as bound.

### Machine reconciliation (P26)

probe-lean's machine `verification-status` stays authoritative on the proof axis;
the blueprint's claim is additive
([P26](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/engineering/properties.md)).
Three derived signals:

- **status-mismatch** (`blueprint-status-mismatch`) — set when the blueprint
  claims the proof done (`proved`/`fully-proved`) but the machine says
  `unverified`/`failed` → `"claims-proved-but-unverified"` /
  `"claims-proved-but-failed"`. Only those two machine states count as a
  contradiction.
- **probe-lean-confirmed** (`theorems-fully-proved-probe-lean-confirmed`) — a
  `fully-proved` **theorem** bound to a present atom and carrying **no**
  status-mismatch. This is a "the machine has not *refuted* this" bar, **not**
  "affirmatively verified": a bound theorem with no `verification-status`, a
  `trusted` one, or one only locally `verified` (sorry-free itself but with an
  unverified dependency) still counts. It never counts a claim the machine
  contradicts, nor an unbound / decl-missing claim (no atom to check). A stricter
  "requires an accepted status such as `transitively-verified`" metric is
  deliberately *not* what this field measures.
- **upstream-proved** (`theorems-fully-proved-upstream-proved`) — a
  `fully-proved` theorem that is decl-missing-upstream-proved: proved
  out-of-workspace per the renderer (see Node classification), so neither
  probe-lean-confirmed locally nor a gap. Surfaced on the human headline as
  `+K upstream-proved`. Always 0 for Massot.

---

## Common: Envelope (Schema 3.x)

Both output files share this envelope structure:

| Field | Type | Description |
|-------|------|-------------|
| `schema` | string | Data type identifier (`"probe-leanblueprint/extract"` or `"probe-leanblueprint/summary"`) |
| `schema-version` | string | Interchange spec version (`"3.0"`) |
| `tool.name` | string | Always `"probe-leanblueprint"` |
| `tool.version` | string | Semver version of the binary |
| `tool.command` | string | Always `"extract"` |
| `source` | Source | Identity of the enriched atom base (propagated from the `probe-lean` input) |
| `timestamp` | string | ISO 8601 timestamp of when the analysis ran |
| `data` | object | Payload (atoms map for `extract`, progress counts for `summary`) |

### Source

| Field | Type | Description |
|-------|------|-------------|
| `repo` | string | Git repository URL |
| `commit` | string | Git commit hash |
| `language` | string | Always `"lean"` |
| `package` | string | Lean package name (overridable with `--source-package`) |
| `package-version` | string | Package version (overridable with `--source-version`) |

The `source` is selected from the atom base's provenance: the `probe-lean/`
input is preferred over an unrelated first `inputs` entry, and this tool's own
`probe-leanblueprint/*` provenance is never treated as a Lean input. See the
[tool KB](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/tools/probe-leanblueprint.md)
for the selection rules.

---

## 1. `probe-leanblueprint/extract` — Enriched Atoms

**Produced by:** `extract`
**Envelope schema:** `"probe-leanblueprint/extract"`
**Category:** Atoms (detected via the `*/extract` suffix, so `probe merge`/`project` accept it)

### Envelope Shape

```json
{
  "schema": "probe-leanblueprint/extract",
  "schema-version": "3.0",
  "tool": {
    "name": "probe-leanblueprint",
    "version": "0.3.0",
    "command": "extract"
  },
  "source": {
    "repo": "https://github.com/Beneficial-AI-Foundation/secure-messaging.git",
    "commit": "4cfee4c1ee6f18d332bbb2dbdc0fc489330447ec",
    "language": "lean",
    "package": "SecureMessaging",
    "package-version": "4cfee4c"
  },
  "timestamp": "2026-07-21T19:28:49Z",
  "data": { ... }
}
```

### Data Shape

`data` is an object keyed by code-name (`probe:` + Lean declaration name). Each
value is a `probe` atom. Atoms that a blueprint node binds are the original
`probe-lean` atoms with `blueprint-*` extension fields added; blueprint nodes
with no present Lean binding become synthetic atoms (see below). The core atom
schema is inherited from `probe-lean/extract`; this tool only **adds** the
`blueprint-*` extensions and synthesizes planned/decl-missing/shadow atoms.

The machine `verification-status` from `probe-lean` stays authoritative on the
proof axis; the blueprint's claim is additive (KB
[P26](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/engineering/properties.md)).

**Bound atom** (a real Lean decl a blueprint node binds — keeps its machine
fields, gains `blueprint-*`):

```json
{
  "probe:AEADScheme": {
    "display-name": "AEADScheme",
    "dependencies": [],
    "code-module": "SecureMessaging.AEAD.Defs",
    "code-path": "SecureMessaging/AEAD/Defs.lean",
    "code-text": { "lines-start": 90, "lines-end": 101 },
    "kind": "structure",
    "language": "lean",
    "verification-status": "transitively-verified",
    "blueprint-label": "aead",
    "blueprint-kind": "definition",
    "blueprint-chapter": "Authenticated-Encryption-with-Associated-Data",
    "blueprint-statement-status": "formalized",
    "blueprint-proof-status": "fully-proved",
    "blueprint-status-source": "code-derived",
    "blueprint-title": "Definition 1.1"
  }
}
```

**Planned-only synthetic atom** (a blueprint node with no Lean binding — the
roadmap layer). `language: "blueprint"` and a non-empty `code-path` marker so P3
stub detection does not misclassify it:

```json
{
  "probe:blueprint:aead_aes_gcm_correctness": {
    "display-name": "aead_aes_gcm_correctness",
    "dependencies": [],
    "code-module": "aead_aes_gcm",
    "code-path": "blueprint",
    "code-text": { "lines-start": 0, "lines-end": 0 },
    "kind": "blueprint-theorem",
    "language": "blueprint",
    "blueprint-label": "aead_aes_gcm_correctness",
    "blueprint-kind": "theorem",
    "blueprint-chapter": "Authenticated-Encryption-with-Associated-Data",
    "blueprint-group": "aead_aes_gcm",
    "blueprint-statement-status": "ready",
    "blueprint-proof-status": "ready",
    "blueprint-status-source": "code-derived",
    "blueprint-title": "Theorem 2.2",
    "blueprint-statement-uses": [
      "probe:blueprint:aead_aes_gcm_spec",
      "probe:AEADScheme.Correct"
    ]
  }
}
```

**Decl-missing synthetic atom** (a node whose *every* bound Lean decl is absent
from the atom base — flagged rather than fabricating a code atom):

```json
{
  "probe:blueprint:ml_kem_scheme": {
    "display-name": "ml_kem_scheme",
    "code-path": "blueprint",
    "kind": "blueprint-definition",
    "language": "blueprint",
    "blueprint-label": "ml_kem_scheme",
    "blueprint-decl-missing": true,
    "blueprint-statement-status": "formalized",
    "blueprint-proof-status": "fully-proved",
    "blueprint-status-source": "code-derived"
  }
}
```

### Blueprint extension fields

Added (flattened) to enriched and synthetic atoms:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `blueprint-label` | string | yes | Blueprint node label |
| `blueprint-kind` | string | yes | Blueprint node kind: `"definition"` or `"theorem"` (lets consumers classify bound atoms whose atom `kind` is the Lean kind) |
| `blueprint-statement-status` | string | yes | Statement axis: `"none"`, `"blocked"`, `"ready"`, or `"formalized"` |
| `blueprint-proof-status` | string | yes | Proof axis: `"none"`, `"ready"`, `"proved"`, or `"fully-proved"` |
| `blueprint-source-statement-status` | string | no | Raw source status when the canonical value is lossy (e.g. Verso `"mathlib"` → `formalized`). Omitted when the mapping is faithful. Additive (see Schema Evolution) |
| `blueprint-source-proof-status` | string | no | Raw source status when the canonical value is lossy (e.g. Verso `"incomplete"` → `none`). Omitted when the mapping is faithful. Additive (see Schema Evolution) |
| `blueprint-status-source` | string | yes | `"code-derived"` (Verso) or `"declared"` (Massot `\leanok`) |
| `blueprint-group` | string | no | Sub-construction grouping label (Verso `parent`) |
| `blueprint-chapter` | string | no | Chapter the node belongs to (one Verso manifest = one chapter) |
| `blueprint-title` | string | no | Display title, e.g. `"Theorem 2.3"` |
| `blueprint-discussion` | string | no | GitHub discussion issue number |
| `blueprint-statement-uses` | array of strings | no | Code-names used by the statement (blueprint labels resolved to real/synthetic atom keys). Extension-only; never merged into `dependencies` |
| `blueprint-proof-uses` | array of strings | no | Code-names used by the proof |
| `blueprint-status-mismatch` | string | no | Set when the blueprint over-claims vs the machine status, e.g. `"claims-proved-but-unverified"` / `"claims-proved-but-failed"` |
| `blueprint-decl-missing` | bool | no | `true` when **all** bound Lean decls are absent (synthetic planned node) |
| `blueprint-decl-upstream-proved` | bool | no | `true` on a decl-missing atom whose every binding is an *out-of-workspace* decl the Verso renderer reports present and proved (proved in a dependency, commonly Mathlib/stdlib but not verified as such — see §Node classification); absent from this project's extract, not a genuine gap. Always paired with `blueprint-decl-missing`. Additive (see Schema Evolution) |
| `blueprint-missing-decls` | array of strings | no | For a bound node, the subset of `\lean{...}` decls absent from the atom base (partial miss); recorded on the present atom(s) |
| `blueprint-shadow` | bool | no | `true` on the synthetic atom preserved for a node that lost a same-decl collision (its real atom was claimed by a later node). Keeps the extract node-complete; count a shadow node as bound despite its `language: "blueprint"` |

Synthetic atoms (`language: "blueprint"`) also carry: `kind` = `"blueprint-<definition|theorem>"`, `code-path` = `"blueprint"`, `code-text` = `{0,0}`, empty `dependencies`, and `code-module` set to the node's group (may be empty). They never carry `verification-status`.

### Two-axis status vocabulary

`blueprint-statement-status` and `blueprint-proof-status` use the canonical
ladders defined in [Semantics → Status axes](#status-axes); the per-adapter
[source-status mapping](#source-status-mapping) shows how each ecosystem's raw
statuses normalize into them.

---

## 2. `probe-leanblueprint/summary` — Progress Sidecar

**Produced by:** `extract`
**Envelope schema:** `"probe-leanblueprint/summary"`
**Category:** None (not an atoms-category file, so it is never merged)

An aggregate over the blueprint nodes (not keyed per node) — the meaningful
two-axis progress stats.

### Envelope + Data Shape

```json
{
  "schema": "probe-leanblueprint/summary",
  "schema-version": "3.0",
  "tool": { "name": "probe-leanblueprint", "version": "0.3.0", "command": "extract" },
  "source": { "language": "lean", "package": "SecureMessaging", "package-version": "4cfee4c", "...": "..." },
  "blueprint-provenance": {
    "adapter": "verso",
    "manifests": [
      { "path": ".../_out/site/.../blueprint-manifest.json",
        "sha256": "a6706fd3...",
        "vbp-internal-schema-version": 3 }
    ]
  },
  "timestamp": "2026-07-21T19:28:49Z",
  "data": {
    "totals": {
      "nodes": 114,
      "with-lean-decl": 29,
      "planned-only": 76,
      "decl-missing": 9,
      "partial-missing": 0,
      "collisions": 0,
      "mismatches": 0
    },
    "all":         { "statement": { "none": 0, "blocked": 64, "ready": 12, "formalized": 38 },
                     "proof":     { "none": 64, "ready": 12, "proved": 1, "fully-proved": 37 } },
    "definitions": { "statement": { "...": 0 }, "proof": { "...": 0 } },
    "theorems":    { "statement": { "...": 0 }, "proof": { "...": 0 } },
    "headline": {
      "theorems-total": 56,
      "theorems-fully-proved": 9,
      "theorems-fully-proved-probe-lean-confirmed": 9,
      "fraction": 0.1607142857142857,
      "fraction-probe-lean-confirmed": 0.1607142857142857
    },
    "by-chapter": {
      "Authenticated-Encryption-with-Associated-Data": {
        "nodes": 14,
        "statement": { "none": 0, "blocked": 0, "ready": 2, "formalized": 12 },
        "proof":     { "none": 0, "ready": 2, "proved": 0, "fully-proved": 12 },
        "theorems-total": 5,
        "theorems-fully-proved": 3
      }
    }
  }
}
```

### `totals`

| Field | Type | Description |
|-------|------|-------------|
| `nodes` | integer | Total blueprint nodes |
| `with-lean-decl` | integer | Nodes bound to at least one present Lean decl (includes collision shadows) |
| `planned-only` | integer | Nodes with no Lean binding at all (roadmap-only) |
| `decl-missing` | integer | Nodes whose every bound decl is absent from the atom base |
| `decl-missing-upstream-proved` | integer | Subset of `decl-missing` proved out-of-workspace per the renderer (present, proved); the rest are genuine gaps. Additive (see Schema Evolution) |
| `partial-missing` | integer | Bound nodes with *some* absent decls (see `blueprint-missing-decls`) |
| `collisions` | integer | Present atoms bound by more than one node (keep-last; losers become shadows) |
| `mismatches` | integer | Nodes whose proof claim contradicts the machine status |

### `all` / `definitions` / `theorems` — AxisCounts

Each is a two-axis histogram over the relevant node subset:

| Field | Type | Description |
|-------|------|-------------|
| `statement.none` / `.blocked` / `.ready` / `.formalized` | integer | Statement-axis histogram |
| `proof.none` / `.ready` / `.proved` / `.fully-proved` | integer | Proof-axis histogram |

`all` counts every node; `definitions` and `theorems` partition by
`blueprint-kind`.

### `headline`

| Field | Type | Description |
|-------|------|-------------|
| `theorems-total` | integer | Number of theorem-kind nodes |
| `theorems-fully-proved` | integer | Theorem nodes the *blueprint* claims `fully-proved`. For `declared` (Massot) blueprints this can over-claim; not a verified-progress number on its own |
| `theorems-fully-proved-probe-lean-confirmed` | integer | Theorem nodes claimed `fully-proved` that probe-lean backs: bound and not contradicted. The honest headline number (P26). Additive (see Schema Evolution) |
| `theorems-fully-proved-upstream-proved` | integer | Fully-proved theorem nodes that are decl-missing here but proved out-of-workspace per the Verso renderer (a dependency, commonly Mathlib/stdlib): neither probe-lean-confirmed locally nor a genuine gap. Surfaced as `+K upstream-proved`. Always 0 for Massot. Additive (see Schema Evolution) |
| `fraction` | float | `theorems-fully-proved / theorems-total` (0.0 when no theorems) |
| `fraction-probe-lean-confirmed` | float | `theorems-fully-proved-probe-lean-confirmed / theorems-total`. Additive (see Schema Evolution) |

### `by-chapter`

An object keyed by chapter name (nodes with no chapter fall under
`"ungrouped"`). Each value extends AxisCounts with:

| Field | Type | Description |
|-------|------|-------------|
| `nodes` | integer | Nodes in this chapter |
| `statement` / `proof` | object | Two-axis histograms (as above) |
| `theorems-total` | integer | Theorem-kind nodes in this chapter |
| `theorems-fully-proved` | integer | Fully-proved theorem nodes in this chapter |

Because the extract is **node-complete** (every model node leaves exactly one
label-bearing record), `scripts/blueprint_stats.py` recomputes these same counts
from the `blueprint-*` fields and agrees with this sidecar exactly (enforced by
a parity test).

---

## Schema Evolution

**Versioning policy.** The emitted `schema-version` mirrors the **ecosystem
interchange version** (currently `3.0`), not a per-tool version — it is set to
whatever the `probe` hub requires so `probe merge`/`project` accept the extract.
It is therefore **not** bumped when this tool adds its own optional fields; it
changes only when the hub's interchange major.minor changes.

Consequently:

- **Additive optional fields** (marked "Additive" in the tables above) are
  `skip-serialized` when absent, so a consumer written for a plainer 3.0 file is
  unaffected, and the hub accepts them under its `3.x` check. They do **not**
  change `schema-version`. Consumers must tolerate unknown `blueprint-*` /
  summary fields.
- **Breaking changes** — removing or renaming a field, or changing the meaning of
  an existing one — are not done unilaterally; they would ride a coordinated
  interchange bump (a new hub major.minor), at which point `schema-version` and
  the fixtures move together.

Consumers should check `schema-version`'s major (`3`) and reject unsupported
majors; they should not key behaviour on a minor, since additive fields ship
without a minor bump.

### Field history (all additive; `schema-version` stayed `3.0`)

Shipped with the move to the 3.0 interchange:

- extract: `blueprint-source-statement-status` / `blueprint-source-proof-status`
  preserve a raw Verso status when the canonical enum is lossy (`mathlib`,
  `incomplete`).
- extract: unknown `source` fields (e.g. `source.class`) round-trip instead of
  being dropped, via the hub `Source` passthrough.
- summary: `theorems-fully-proved-probe-lean-confirmed` / `fraction-probe-lean-confirmed`
  report progress probe-lean actually backs, distinct from the blueprint's own claim.
- summary: a top-level `blueprint-provenance` block (adapter; for Verso each
  manifest's `path`, `sha256`, `vbp-internal-schema-version`; `web-tex` for Massot).

Added later (still `3.0`):

- the out-of-workspace-proved decl split — `blueprint-decl-upstream-proved`
  (atom), `decl-missing-upstream-proved` (totals), and
  `theorems-fully-proved-upstream-proved` (headline) — separating a decl-missing
  node proved out-of-workspace (per the Verso renderer) from a genuine gap.

---

## Compatibility

### With probe-lean

`probe-leanblueprint` consumes a `probe-lean/extract` atom base (via `--lean`,
or by running `probe-lean extract` itself). It preserves all `probe-lean` atom
fields verbatim and only adds `blueprint-*` extensions. A merged spine
(`probe/merged-*`) carrying old blueprint atoms is also accepted — prior
enrichment is scrubbed first, so re-runs are idempotent. Passing this tool's own
`probe-leanblueprint/extract` back in is rejected (self-ingestion).

probe-lean >= v0.10.0 emits interchange `schema-version` 3.0 and is consumed
directly. Older releases (<= v0.9.6) and extracts already on disk emit 2.x; the
tool re-stamps a 2.x `probe-lean/extract` to 3.0 in a temp copy before loading
(the original file is left unchanged; a pure `schema-version` bump — probe-lean's
atom fields are unchanged across 2->3, guarded by refusing if any atom carries
the renamed `is-disabled` field), so no re-extraction is required. A 2.x input of
another schema (e.g. a `probe/merged-*` spine) is not auto-migrated and errors
with guidance to re-extract or migrate first.

### With probe merge / project

The `probe-leanblueprint/extract` file is an atoms-category Schema 3.0 envelope,
so `probe merge`/`probe project` accept it and preserve the `blueprint-*`
extension fields (KB
[P10](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/engineering/properties.md)).
The `probe-leanblueprint/summary` sidecar is not an atoms category and is never
merged.

### With the probe (shared) crate

`probe-leanblueprint` depends on the `probe` hub crate for shared types
(`Atom`, `AtomEnvelope`, `Source`, `Tool`, `CodeText`, `load_atom_file`) and for
`probe::commands::propagate::enrich_verification_status` (reused, idempotent).
See the
[tool KB](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/tools/probe-leanblueprint.md)
and [ADR-004](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/decisions/004-probe-leanblueprint.md)
for the full design.
