# probe-leanblueprint Data Schemas

Schema version: 3.0
Date: 2026-07-21

This document specifies the JSON output formats produced by `probe-leanblueprint`.
It complements the language-agnostic
[envelope-rationale.md](https://github.com/Beneficial-AI-Foundation/probe/blob/main/docs/envelope-rationale.md),
which defines the envelope wrapper; this document defines what goes **inside**
the `data` field of each output file.

`probe-leanblueprint` is an **enricher**: it consumes a `probe-lean/extract`
atom base and a blueprint (Verso manifest or Massot LaTeX), joins them by Lean
declaration name, and re-emits two files:

- `probe-leanblueprint/extract` — the enriched atoms (an atoms-category file,
  so `probe merge`/`project` accept it and preserve the `blueprint-*` fields).
- `probe-leanblueprint/summary` — a two-axis progress sidecar (never merged).

Both are produced by the single `extract` subcommand.

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
| `blueprint-source-statement-status` | string | no | Raw source status when the canonical value is lossy (e.g. Verso `"mathlib"` → `formalized`). Omitted when the mapping is faithful. Added in 3.0 |
| `blueprint-source-proof-status` | string | no | Raw source status when the canonical value is lossy (e.g. Verso `"incomplete"` → `none`). Omitted when the mapping is faithful. Added in 3.0 |
| `blueprint-status-source` | string | yes | `"code-derived"` (Verso) or `"declared"` (Massot `\leanok`) |
| `blueprint-group` | string | no | Sub-construction grouping label (Verso `parent`) |
| `blueprint-chapter` | string | no | Chapter the node belongs to (one Verso manifest = one chapter) |
| `blueprint-title` | string | no | Display title, e.g. `"Theorem 2.3"` |
| `blueprint-discussion` | string | no | GitHub discussion issue number |
| `blueprint-statement-uses` | array of strings | no | Code-names used by the statement (blueprint labels resolved to real/synthetic atom keys). Extension-only; never merged into `dependencies` |
| `blueprint-proof-uses` | array of strings | no | Code-names used by the proof |
| `blueprint-status-mismatch` | string | no | Set when the blueprint over-claims vs the machine status, e.g. `"claims-proved-but-unverified"` / `"claims-proved-but-failed"` |
| `blueprint-decl-missing` | bool | no | `true` when **all** bound Lean decls are absent (synthetic planned node) |
| `blueprint-missing-decls` | array of strings | no | For a bound node, the subset of `\lean{...}` decls absent from the atom base (partial miss); recorded on the present atom(s) |
| `blueprint-shadow` | bool | no | `true` on the synthetic atom preserved for a node that lost a same-decl collision (its real atom was claimed by a later node). Keeps the extract node-complete; count a shadow node as bound despite its `language: "blueprint"` |

Synthetic atoms (`language: "blueprint"`) also carry: `kind` = `"blueprint-<definition|theorem>"`, `code-path` = `"blueprint"`, `code-text` = `{0,0}`, empty `dependencies`, and `code-module` set to the node's group (may be empty). They never carry `verification-status`.

### Two-axis status vocabulary (canonical)

| Axis | Ordered worst → best | Meaning |
|------|----------------------|---------|
| statement | `none` → `blocked` → `ready` → `formalized` | Is the *statement* formalized in Lean? |
| proof | `none` → `ready` → `proved` → `fully-proved` | Is the *proof* complete (sorry-free)? |

Both Verso and Massot source statuses are normalized into this vocabulary; see
the [tool KB](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/tools/probe-leanblueprint.md#two-axis-status-vocabulary-canonical)
for the full mapping table.

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
      "theorems-fully-proved-machine-confirmed": 9,
      "fraction": 0.1607142857142857,
      "fraction-machine-confirmed": 0.1607142857142857
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
| `theorems-fully-proved-machine-confirmed` | integer | Theorem nodes claimed `fully-proved` that probe-lean backs: bound and not contradicted. The honest headline number (P26). Added in 3.0 |
| `fraction` | float | `theorems-fully-proved / theorems-total` (0.0 when no theorems) |
| `fraction-machine-confirmed` | float | `theorems-fully-proved-machine-confirmed / theorems-total`. Added in 3.0 |

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

When adding new optional fields, increment the minor version. When changing
required fields or their semantics, increment the major version.

Consumers should check `schema-version` and reject files with an unsupported
major version.

### 3.0

Aligns `schema-version` with the ecosystem-wide interchange bump to 3.0 (so
`probe merge`/`project` accept the extract), and adds:

- extract: optional `blueprint-source-statement-status` / `blueprint-source-proof-status`
  preserve a raw Verso status when the canonical enum is lossy (`mathlib`,
  `incomplete`).
- extract: unknown `source` fields (e.g. `source.class`) now round-trip instead
  of being dropped, via the hub `Source` passthrough.
- summary: `theorems-fully-proved-machine-confirmed` and `fraction-machine-confirmed`
  report progress probe-lean actually backs, distinct from the blueprint's own claim.
- summary: a top-level `blueprint-provenance` block records which blueprint inputs
  produced the output — the `adapter`, and for Verso each manifest's `path`,
  `sha256`, and `vbp-internal-schema-version` (or `web-tex` for Massot).

---

## Compatibility

### With probe-lean

`probe-leanblueprint` consumes a `probe-lean/extract` atom base (via `--lean`,
or by running `probe-lean extract` itself). It preserves all `probe-lean` atom
fields verbatim and only adds `blueprint-*` extensions. A merged spine
(`probe/merged-*`) carrying old blueprint atoms is also accepted — prior
enrichment is scrubbed first, so re-runs are idempotent. Passing this tool's own
`probe-leanblueprint/extract` back in is rejected (self-ingestion).

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
