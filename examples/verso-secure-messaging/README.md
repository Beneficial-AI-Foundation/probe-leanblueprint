# Example: Secure Messaging (Verso blueprint)

A real, multi-chapter research blueprint: the Lean formalization of cryptographic
primitives and protocols for secure messaging. Unlike the other examples (each a
single Verso manual), this project renders **one manual per chapter**, so the
blueprint is nine per-chapter `blueprint-manifest.json` files that
`probe-leanblueprint` discovers and merges.

- **Repo (Lean + blueprint):** [`Beneficial-AI-Foundation/secure-messaging`](https://github.com/Beneficial-AI-Foundation/secure-messaging) @ `6a4fce0`

| | |
|---|---|
| Lean toolchain | `v4.30.0` |
| verso-blueprint | `v4.30.0` (`vbpInternalSchemaVersion` 2) |
| Math library | `SecureMessaging` |
| Render entrypoint | per-chapter, via [`scripts/render-docs-site.sh`](https://github.com/Beneficial-AI-Foundation/secure-messaging/blob/main/scripts/render-docs-site.sh) |
| `source.class` | `security-protocol` |

## Reproduce

This project renders each chapter as its own Verso manual, so it does not use the
single-manual [`../reproduce-verso-example.sh`](../reproduce-verso-example.sh).
From a checkout at commit `6a4fce0`:

```bash
lake exe cache get && lake build          # mathlib/VCVio + the library
scripts/render-docs-site.sh                # -> _out/site/html-multi/<Chapter>/-verso-data/blueprint-manifest.json  (×9)

probe-leanblueprint extract . \
    --verso-manifest _out/site \           # discovers + merges all 9 chapter manifests
    --lean <probe-lean-v4.30.0 extract of SecureMessaging>
```

Alternatively, let `probe-leanblueprint` drive the render itself with
`--verso-render-cmd` instead of running `scripts/render-docs-site.sh` as a
separate step. This project renders per-chapter, so the custom script overrides
the `lake exe vbp build` default:

```bash
lake exe cache get && lake build          # mathlib/VCVio + the library

probe-leanblueprint extract . \
    --verso-render-cmd scripts/render-docs-site.sh \   # renders, then discovers + merges under _out/site
    --lean <probe-lean-v4.30.0 extract of SecureMessaging>
```

The artifacts here are regenerated from this repo's committed fixtures — the nine
chapter manifests under
[`tests/fixtures/verso/secure-messaging/`](../../tests/fixtures/verso/secure-messaging/)
and the same-commit atom base
[`tests/fixtures/lean/secure-messaging-atoms.json`](../../tests/fixtures/lean/secure-messaging-atoms.json)
(trimmed to the blueprint-bound decls with their real `verification-status`).
Because the render and the atoms are from the *same* commit, every bound decl is
present — `decl-missing` is 0 by construction.

## Result

Full report: [`blueprint-stats.txt`](./blueprint-stats.txt) · machine summary:
[`extract.summary.json`](./extract.summary.json).

```
Headline: 8/53 theorems probe-lean-confirmed fully proved (15.1%)
Blueprint nodes: 111   (bound 33 · planned-only 78 · decl-missing 0 · partial-missing 0 · mismatches 0)
```

- **58 definitions + 53 theorems = 111 nodes** across 9 chapters. The status is
  code-derived (Verso), so probe-lean-confirmed equals the blueprint's own claim —
  there are **0 mismatches**.
- **78 planned-only** nodes: this is an early-stage formalization with a large
  roadmap of not-yet-bound results (65 statements still `blocked`).
- `source.class: "security-protocol"` from `probe-lean` round-trips into the
  output `source` (schema 3.0 passthrough).

## Comparison against Verso's own rendering

The per-chapter counts match the deployed per-chapter **Blueprint-Summary**
pages — e.g. Authenticated-Encryption-with-Associated-Data reports 14 nodes, 5
theorems, 3 fully proved, which is exactly this example's AEAD row. All 111 graph
nodes are represented (the extract is node-complete), and the two-axis tallies
reproduce the merged manifests' raw status counts:

| Proof axis | count | Statement axis | count |
|---|---|---|---|
| `fully-proved` | 33 | `formalized` | 33 |
| `ready` | 13 | `ready` | 13 |
| `none` | 65 | `blocked` | 65 |

**What probe-leanblueprint adds:** it merges the nine per-chapter manifests into
one model (de-duplicating cross-chapter node references), joins the 33 bound
nodes onto real `probe-lean` atoms, and cross-checks every proof claim against
probe-lean's sorry-free status — **0 mismatches**.
