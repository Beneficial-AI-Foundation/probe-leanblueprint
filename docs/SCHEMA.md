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
a Lean decl present in the atom base. Whatever the bucket, the node **also**
leaves exactly one [node atom](#node-atoms) (`language: "blueprint"`, keyed
`probe:blueprint:<label>`, discriminated by `blueprint-node-class`), so the
extract carries one per-node record per Verso node:

- **bound** (`with-lean-decl`) — binds ≥1 present Lean decl. The present atom(s)
  gain the `blueprint-*` fields, and the node's node atom (`blueprint-node-class:
  "bound"`) aggregates the whole binding.
- **planned-only** — binds no Lean decl at all (roadmap only). Represented only
  by its node atom.
- **decl-missing** — binds ≥1 Lean decl but *every* one is absent from the atom
  base. Represented only by its node atom, flagged `blueprint-decl-missing`.
  Split further:
  - **upstream-proved** — *every* binding is an external decl the Verso renderer
    reports as **out-of-workspace** (its `provenance.outWorkspace`) **and**
    present **and** proved: proved elsewhere, absent here — not a genuine gap.
    "Out-of-workspace" is exactly what's checked (a decl in some dependency built
    on, *commonly* Mathlib/stdlib but not verified to be — could be any
    out-of-workspace package); "upstream" is shorthand for that, not a namespace
    claim. Flagged `blueprint-decl-upstream-proved`. code-derived only (Massot
    carries no per-decl provenance).
  - **genuine gap** — everything else decl-missing (at least one binding is absent
    and not upstream-proved). Such a node is *not* flagged
    `blueprint-decl-upstream-proved`, but if *some* of its bindings are
    upstream-proved those are still listed in `blueprint-upstream-decls`.
- **partial-missing** — a *bound* node where some (not all) decls are absent from
  the atom base, **excluding** decls the renderer proved out-of-workspace (an
  upstream decl is expected to be absent here — it lives in a dependency — so it
  is not a gap). The genuinely-absent names go in `blueprint-missing-decls` on the
  present atom(s); any excluded upstream decls are recorded in
  `blueprint-upstream-decls` instead. A node whose only absent decls are all
  upstream-proved is therefore **not** partial-missing — its whole binding is
  accounted for (present locally or proved upstream).
- **collision-shadow** — a *bound* node whose every present decl was claimed by
  a later node (keep-last): its node atom is then its only label-bearing record,
  additionally flagged `blueprint-shadow`. Counts as bound.

### Node atoms

Exactly one `language: "blueprint"` atom per blueprint node, keyed
`probe:blueprint:<label>` — so `#node-atoms` equals the blueprint's node count
(the summary's `totals.node-atoms` invariant). Each carries the node's
`blueprint-*` fields, a `blueprint-node-class` discriminator (`"bound"` /
`"planned-only"` / `"decl-missing"`), and a derived `verification-status` (see
[Derived verification-status](#derived-verification-status-node-atoms)). A
*bound* node atom also carries its present decls as `dependencies` — the
node→code mapping, and what keeps its derived status stable under a later
`probe enrich`.

**Uses resolution is class-dependent.** On a **node atom**,
`blueprint-statement-uses`/`blueprint-proof-uses` resolve node-to-node (each
used label → that label's node-atom key), so the node atoms plus their uses
edges form a **closed per-node graph** matching the Verso blueprint. On an
**enriched real atom**, the same fields keep the historical resolution (each
used label → its primary code representative: the first present atom it owns,
else its node-atom key) — enriched real atoms are byte-compatible with
pre-node-atom output.

**Which layer to read** (for consumers such as the VeriLib frontend):

- *Code-level* stats, coloring, and the code dependency graph → `language:
  "lean"` atoms only.
- *Blueprint-level* progress, per-node statuses, and the paper graph →
  node atoms (`probe:blueprint:*`) only.
- *Connectivity between the layers* → `blueprint-label` on a real atom names
  its node (append to `probe:blueprint:` for the node-atom key); a bound node
  atom's `dependencies` list its real atoms. **These two maps are not inverses
  under collisions**: `dependencies` means *claimed binding* (a collision
  loser still lists the decl it lost), while `blueprint-label` means
  *ownership winner* (keep-last) — traversing decl → label → node atom reaches
  only the winning node.
- **Never sum statuses or trust bases across both layers**: a bound node atom's
  derived status mirrors decls already counted on the lean side. This applies
  to generic hub consumers too — e.g. `probe summary` counts verified
  non-Rust atoms as verified lemmas, so bound node atoms inflate such counts
  unless `language: "blueprint"` is filtered out, and `probe project` reverse
  traversal can pull node atoms in via their `dependencies`. Until the hub
  learns to exclude blueprint-language atoms natively, generic-consumer runs
  over this extract should filter them first.

### Machine reconciliation (P26)

probe-lean's machine `verification-status` stays authoritative on the proof axis;
the blueprint's claim is additive
([P26](https://github.com/Beneficial-AI-Foundation/probe/blob/main/kb/engineering/properties.md)).
Three derived signals:

- **status-mismatch** (`blueprint-status-mismatch`) — set when the blueprint
  claims the proof done (`proved`/`fully-proved`) but the machine says
  `unverified`/`failed` → `"claims-proved-but-unverified"` /
  `"claims-proved-but-failed"`. Only those two machine states count as a
  contradiction, and only **present** atoms are checked (the first offending
  one names the marker) — a decl-missing or partial-missing claim carries no
  mismatch, and the marker is deliberately narrower than a node atom's derived
  `verification-status`, which aggregates the whole binding.
- **probe-lean-confirmed** (`theorems-fully-proved-probe-lean-confirmed`) — a
  `fully-proved` **theorem** bound to a present atom, carrying **no**
  status-mismatch, and whose *whole* binding is accounted for — i.e. it is **not**
  partial-missing: every bound decl is either present in the atom base or proved
  out-of-workspace by the renderer. A **mixed** node (a present local decl plus an
  absent upstream-proved decl) therefore *does* qualify; its upstream portion is
  recorded on the wire in `blueprint-upstream-decls` so a consumer can tell mixed
  backing from fully-local. This is a "the machine has not *refuted* this" bar,
  **not** "affirmatively verified": a bound theorem with no `verification-status`,
  a `trusted` one, or one only locally `verified` (sorry-free itself but with an
  unverified dependency) still counts. It never counts a claim the machine
  contradicts, a partial-missing claim (a genuinely-absent decl the machine can't
  back), nor an unbound / fully-decl-missing claim (no atom to check). A stricter
  "requires an accepted status such as `transitively-verified`" metric is
  deliberately *not* what this field measures.
- **upstream-proved** (`theorems-fully-proved-upstream-proved`) — a
  `fully-proved` theorem that is decl-missing-upstream-proved: proved
  out-of-workspace per the renderer (see Node classification), so neither
  probe-lean-confirmed locally nor a gap. Surfaced on the human headline as
  `+K upstream-proved`. Always 0 for Massot.

### Derived verification-status (node atoms)

Real atoms carry probe-lean's machine `verification-status`, which this tool
never modifies (P26). Node atoms (`language: "blueprint"`) are per-node
records — instead the `extract` CLI **derives** a `verification-status` (plus
`trusted-reason` where trust is attested) so the field is total across the
node atoms it emits. (It is not total across the whole file in general: a
`--skip-verify` atom base leaves real atoms status-less, and P26 forbids
inventing statuses for them. The library `enrich()` join alone does not stamp
node atoms either; the derivation is a separate pass the CLI runs after the
hub's transitive propagation.)

Each node class derives from the strongest evidence available. The governing
rule is that **machine-vocabulary values (`verified`/`transitively-verified`)
are only ever inherited from decls probe-lean actually checked; claims — human
or renderer — cap at `trusted`; and losing binding evidence can never improve
a status.**

| Node atom | Derived status | Who vouches |
|-----------|----------------|-------------|
| bound (collision shadows included) | aggregate over the node's **whole binding** (see below) | probe-lean checked the present decls *here* |
| decl-missing, `blueprint-decl-upstream-proved` | `"trusted"`, `trusted-reason: "upstream-proved"` | the Verso renderer proved every binding out-of-workspace |
| decl-missing or planned-only, `declared` source, claimed proof `proved`/`fully-proved` | `"trusted"`, `trusted-reason: "declared"` | a human `\leanok` (possibly proven in another repo; nothing here checked it) |
| everything else — including a planned-only `code-derived` node whatever its claimed proof status | `"unverified"` | nobody |

A code-derived proved/fully-proved claim on a *planned-only* node is a
contradiction (the Verso renderer requires associated code to judge a proof),
i.e. likely manifest drift or lost preview linkage; it derives `"unverified"`
and warns, so degraded binding evidence surfaces instead of minting a
machine-looking status.

**Binding aggregation (bound node atoms).** A bound node atom's status covers
its whole binding, one *component* per bound decl: each present decl
contributes its final (post-propagation) machine status, each
genuinely-missing decl contributes `unverified`, and each upstream-proved
absent decl contributes `trusted`. Aggregation is failure-first and
trust-sticky: any `failed` component → `failed`; else any
`unverified`/absent/unknown component → `unverified`; else any `trusted`
component → `trusted` (an attestation anywhere in the binding caps the whole
at attested); else any `verified` → `verified`; else `transitively-verified`.
When the result is `trusted`, `trusted-reason` is emitted only if the
contributing trusted components agree on a single reason (present decls' own
reasons, plus `"upstream-proved"` for upstream components); disagreement omits
it. A bound node atom's status (and any copied reason) *mirrors* decls already
counted on their real atoms — hence the layer-split rule under
[Node atoms](#node-atoms): never aggregate statuses across both layers.

**Ordering and stability.** The derivation runs **after** the hub's
transitive-verification propagation: a bound node atom aggregates *final*
machine statuses, and nothing the derivation writes is visible to that pass.
Carrying present bindings as `dependencies` (the one exception to the
empty-`dependencies` rule for node atoms) closes one specific hole under a
*later* `probe enrich` over the emitted file: a derived `verified` is judged
against the real closure instead of being vacuously upgraded over an empty
one. It is **not** a general recomputation guarantee — the hub never
downgrades a stale `transitively-verified` and never upgrades
`unverified`/`failed`/`trusted`, so on a merged or edited graph a node atom's
status reflects extract time, not the merged state.
Edges point node atom → real only; within this tool's own output no real atom
depends on a node atom (`blueprint-*-uses` edges are extension-only, never
merged into `dependencies`), so derived statuses cannot leak into real-atom
propagation and the blast radius of an over-claimed source status is exactly
the one node atom carrying it. (That no-feedback property holds for edges this
tool emits; it is not re-validated for arbitrary merged input.)

`trusted-reason` extends probe-lean's reason vocabulary (`axiom`,
`externally_verified`, `external`) with two additive values: `upstream-proved`
and `declared`. As in probe-lean, it is present only when the status is
`"trusted"` (or inherited from a `trusted` binding by a bound node atom).

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
`probe-lean` atoms with `blueprint-*` extension fields added; additionally,
**every** blueprint node — bound or not — leaves exactly one
[node atom](#node-atoms) keyed `probe:blueprint:<label>`. The core atom schema
is inherited from `probe-lean/extract`; this tool only **adds** the
`blueprint-*` extensions and synthesizes the node atoms.

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

**Planned-only node atom** (a blueprint node with no Lean binding — the
roadmap layer). `language: "blueprint"`, filed under its chapter's virtual
folder (`code-path` non-empty, so P3 stub detection does not misclassify it):

```json
{
  "probe:blueprint:aead_aes_gcm_correctness": {
    "display-name": "aead_aes_gcm_correctness",
    "dependencies": [],
    "code-module": "Blueprint.Authenticated-Encryption-with-Associated-Data.aead_aes_gcm",
    "code-path": "blueprint/Authenticated-Encryption-with-Associated-Data",
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
    "blueprint-node-class": "planned-only",
    "blueprint-statement-uses": [
      "probe:blueprint:aead_aes_gcm_spec",
      "probe:blueprint:aead"
    ],
    "verification-status": "unverified"
  }
}
```

The `verification-status` on a node atom is **derived** (see
[Semantics → Derived verification-status](#derived-verification-status-node-atoms)),
not a probe-lean machine judgment — and note the node-to-node uses resolution
(`probe:blueprint:aead`, not the real atom `probe:AEADScheme` that the *lean*
atom's uses field would reference).

**Bound node atom** (the per-node record of a bound node — aggregates its
whole binding):

```json
{
  "probe:blueprint:aead": {
    "display-name": "aead",
    "dependencies": ["probe:AEADScheme"],
    "code-module": "Blueprint.Authenticated-Encryption-with-Associated-Data",
    "code-path": "blueprint/Authenticated-Encryption-with-Associated-Data",
    "code-text": { "lines-start": 0, "lines-end": 0 },
    "kind": "blueprint-definition",
    "language": "blueprint",
    "blueprint-label": "aead",
    "blueprint-kind": "definition",
    "blueprint-node-class": "bound",
    "blueprint-chapter": "Authenticated-Encryption-with-Associated-Data",
    "blueprint-statement-status": "formalized",
    "blueprint-proof-status": "fully-proved",
    "blueprint-status-source": "code-derived",
    "blueprint-title": "Definition 1.1",
    "verification-status": "transitively-verified"
  }
}
```

**Decl-missing node atom** (a node whose *every* bound Lean decl is absent
from the atom base — flagged rather than fabricating a code atom):

```json
{
  "probe:blueprint:ml_kem_scheme": {
    "display-name": "ml_kem_scheme",
    "code-module": "Blueprint.Key-Encapsulation-Mechanism",
    "code-path": "blueprint/Key-Encapsulation-Mechanism",
    "blueprint-chapter": "Key-Encapsulation-Mechanism",
    "kind": "blueprint-definition",
    "language": "blueprint",
    "blueprint-label": "ml_kem_scheme",
    "blueprint-decl-missing": true,
    "blueprint-statement-status": "formalized",
    "blueprint-proof-status": "fully-proved",
    "blueprint-status-source": "code-derived",
    "verification-status": "unverified"
  }
}
```

(Genuine-gap decl-missing derives `"unverified"` even under a `fully-proved`
claim — a claim with no checkable code behind it must not mint a
machine-looking status. The upstream-proved variant derives
`"trusted"` / `trusted-reason: "upstream-proved"` instead.)

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
| `blueprint-statement-uses` | array of strings | no | Code-names used by the statement. Resolution is class-dependent (see [Node atoms → Uses resolution](#node-atoms)): node-to-node on a node atom, code representatives on an enriched real atom. Extension-only; never merged into `dependencies` |
| `blueprint-proof-uses` | array of strings | no | Code-names used by the proof (same class-dependent resolution) |
| `blueprint-status-mismatch` | string | no | Set when the blueprint over-claims vs the machine status, e.g. `"claims-proved-but-unverified"` / `"claims-proved-but-failed"` |
| `blueprint-decl-missing` | bool | no | `true` when **all** bound Lean decls are absent (synthetic planned node) |
| `blueprint-decl-upstream-proved` | bool | no | `true` on a decl-missing atom whose every binding is an *out-of-workspace* decl the Verso renderer reports present and proved (proved in a dependency, commonly Mathlib/stdlib but not verified as such — see §Node classification); absent from this project's extract, not a genuine gap. Always paired with `blueprint-decl-missing`. Additive (see Schema Evolution) |
| `blueprint-missing-decls` | array of strings | no | For a bound node, the subset of `\lean{...}` decls absent from the atom base (partial miss), **excluding** upstream-proved decls (those go in `blueprint-upstream-decls`); recorded on the present atom(s) |
| `blueprint-upstream-decls` | array of strings | no | The node's bound decls that are **absent from the atom base** but the Verso renderer proved **out-of-workspace** (present + proved in a dependency) — i.e. the part of the binding backed upstream rather than by local probe-lean. Never includes a locally-present decl, and is always disjoint from `blueprint-missing-decls`. On a *bound* atom it marks a **mixed** binding (part local, part upstream): the node stays probe-lean-confirmed and these decls are kept out of `blueprint-missing-decls`, so this is the wire evidence distinguishing mixed from fully-local backing. On a **decl-missing** atom it lists the upstream part of the binding — present together with `blueprint-decl-upstream-proved` when *every* binding is upstream, or on its own (no bool) when only *some* are (a partial gap). code-derived (Verso) only. Additive (see Schema Evolution) |
| `blueprint-shadow` | bool | no | `true` on the node atom of a bound node that lost a same-decl collision (its every real atom was claimed by a later node, so the node atom is its only label-bearing record). Count a shadow node as bound |
| `blueprint-node-class` | string | no | Node-atom class discriminator: `"bound"`, `"planned-only"`, or `"decl-missing"`. Present on every node atom, never on an enriched real atom (whose bytes are frozen). Additive (see Schema Evolution) |

Node atoms (`language: "blueprint"`) also carry: `kind` = `"blueprint-<definition|theorem>"`, `code-path` = `"blueprint/<chapter-slug>"` (the node's virtual location — one `blueprint/` tree, one folder per chapter; `blueprint/ungrouped` when the blueprint gives no chapter or its slug is empty; always non-empty, so P3 stub detection never fires), `code-module` = `"Blueprint.<chapter-slug>[.<group-slug>]"` (the dotted-module analogue; the group level appears exactly when the node has a group whose slug is non-empty — a group literally named `ungrouped` keeps its level), `code-text` = `{0,0}`, and empty `dependencies` (except a **bound** node atom, which carries its present bindings as dependencies — see [Node atoms](#node-atoms)).

Slug rules: alphanumerics (unicode included), `_` and `-` pass through; every other character collapses into a single `-`; components are capped (64 bytes) and never empty. A slug is a **display grouping, not an identity**: distinct raw names may share a slug (`A B` and `A.B` both give `A-B`), and the values preserved in `blueprint-chapter` / `blueprint-group` are the names **as the adapter provides them** — the Massot emitter forwards the LaTeX sectioning title, while a Verso manifest carries its own href-derived slug (a unicode chapter like `µCMZ` may already arrive flattened by Verso). Consumers needing exact identities must key on those extension fields, not on the location slugs. In CLI output they carry a **derived** `verification-status` (plus `trusted-reason` where applicable) computed per [Semantics → Derived verification-status](#derived-verification-status-node-atoms) — unlike a real atom's machine status, it reflects binding aggregation and blueprint-side evidence, never a fresh local probe-lean check.

#### Derived core fields on node atoms

These are **core atom keys**, not `blueprint-*` extensions: they are deliberately
absent from the re-enrichment scrub list (clearing them from real atoms would
destroy probe-lean's machine data; node atoms are instead rebuilt wholesale
on every run), and their meaning is class-dependent — on `language: "lean"`
atoms `verification-status` is probe-lean's machine judgment, untouched by this
tool (P26); on `language: "blueprint"` atoms it is derived by the `extract` CLI.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `verification-status` | string | yes (CLI output; the library `enrich()` join alone does not stamp it) | **Derived**, per [Semantics → Derived verification-status](#derived-verification-status-node-atoms): binding aggregation and blueprint-side evidence, never a fresh local probe-lean check. Values: `"unverified"`, `"trusted"`, or — bound node atoms only, aggregated — `"failed"`, `"verified"`, `"transitively-verified"` |
| `trusted-reason` | string | no | Present only when the status is `"trusted"`: `"upstream-proved"` (renderer proved every binding out-of-workspace), `"declared"` (human `\leanok` claim), or a reason copied verbatim from a trusted binding by a bound node atom (e.g. `"axiom"`). Omitted when the trusted components disagree on a reason |

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
two-axis progress stats. `nodes` counts **unique labels after cross-manifest
merging** (a label legitimately recurs across per-chapter Verso manifests as a
mention; see the model merge policy in `src/model.rs`), which is what "the
blueprint's node count" means throughout this document.

### Envelope + Data Shape

> The example below was generated by an earlier release (pre-`node-atoms`);
> fields added since are documented in the tables and marked Additive, and the
> committed example artifacts under `examples/` will pick them up on their next
> regeneration.

```json
{
  "schema": "probe-leanblueprint/summary",
  "schema-version": "3.0",
  "tool": { "name": "probe-leanblueprint", "version": "0.3.0", "command": "extract" },
  "source": { "language": "lean", "package": "SecureMessaging", "package-version": "6a4fce0", "class": "security-protocol", "...": "..." },
  "blueprint-provenance": {
    "adapter": "verso",
    "manifests": [
      { "path": ".../_out/site/.../blueprint-manifest.json",
        "sha256": "72be94e5...",
        "vbp-internal-schema-version": 3 }
    ]
  },
  "timestamp": "2026-07-21T19:28:49Z",
  "data": {
    "totals": {
      "nodes": 111,
      "with-lean-decl": 33,
      "planned-only": 78,
      "decl-missing": 0,
      "decl-missing-upstream-proved": 0,
      "partial-missing": 0,
      "collisions": 0,
      "mismatches": 0
    },
    "all":         { "statement": { "none": 0, "blocked": 65, "ready": 13, "formalized": 33 },
                     "proof":     { "none": 65, "ready": 13, "proved": 0, "fully-proved": 33 } },
    "definitions": { "statement": { "...": 0 }, "proof": { "...": 0 } },
    "theorems":    { "statement": { "...": 0 }, "proof": { "...": 0 } },
    "headline": {
      "theorems-total": 53,
      "theorems-fully-proved": 8,
      "theorems-fully-proved-probe-lean-confirmed": 8,
      "theorems-fully-proved-upstream-proved": 0,
      "fraction": 0.1509433962264151,
      "fraction-probe-lean-confirmed": 0.1509433962264151
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
| `node-atoms` | integer | Node atoms emitted (`language: "blueprint"`, one per node) — the checkable invariant against the blueprint's node count: equals `nodes` unless a duplicate label or synthetic-key collision dropped one (both warn). Additive (see Schema Evolution); the committed example artifacts predate it |

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
- extract: `blueprint-upstream-decls` — the node's bound decls that are absent
  from the atom base but proved out-of-workspace, distinguishing a mixed
  (part-local, part-upstream) binding from a fully-local one.
- extract: synthetic (`language: "blueprint"`) atoms now carry a **derived**
  `verification-status` (plus `trusted-reason: "upstream-proved" | "declared"`
  where trust is attested), making the field total across the CLI's synthetic
  atoms — see
  [Semantics → Derived verification-status](#derived-verification-status-node-atoms).
  Collision shadows additionally carry their present bindings as
  `dependencies` (stability under downstream `probe enrich`). Real atoms'
  machine statuses are unchanged (P26). Previously synthetic atoms carried no
  `verification-status` at all; consumers keying on the field's absence to
  detect synthetics should key on `language: "blueprint"` instead, and
  consumers aggregating statuses across atoms should exclude
  `blueprint-shadow` atoms (their status mirrors decls counted elsewhere).
- extract: **node atoms** — every blueprint node (bound included) now leaves
  exactly one `language: "blueprint"` atom keyed `probe:blueprint:<label>`,
  discriminated by the new `blueprint-node-class` field, so `#node-atoms`
  matches the blueprint's node count (see [Node atoms](#node-atoms)); summary
  `totals` gains `node-atoms`. Bound node atoms aggregate their whole binding
  into the derived `verification-status` and carry their present decls as
  `dependencies`; the earlier shadow-only synthetic is now simply the bound
  node atom of a collision loser. Enriched real atoms are byte-identical to
  pre-node-atom output. **One value change** rides this addition: on node
  atoms (including pre-existing planned-only/decl-missing ones),
  `blueprint-*-uses` now resolve node-to-node (previously to the used label's
  primary code representative); the resolution on enriched real atoms is
  unchanged. Consumers reading uses edges off `language: "blueprint"` atoms
  must expect `probe:blueprint:*` targets. The status double-count guidance
  extends from shadows to all bound node atoms: never aggregate statuses
  across the lean and node layers.

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
