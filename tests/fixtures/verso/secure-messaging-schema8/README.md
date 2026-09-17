# secure-messaging, schema-8 unified manifest (projection)

`blueprint-manifest.json` is a projection of the deployed verso-blueprint
manifest for [`Beneficial-AI-Foundation/secure-messaging`](https://github.com/Beneficial-AI-Foundation/secure-messaging)
to the fields the Verso adapter reads, produced by `project.jq` in this
directory. The committed projection is the artifact the tests depend on; the
URL is a moving deployment.

| | |
|---|---|
| URL | `https://beneficial-ai-foundation.github.io/secure-messaging/-verso-data/blueprint-manifest.json` |
| fetched | 2026-09-17 |
| sha256 (raw download) | `dd1712fc76ff1b6cda20cfe14fb84aae12d07bf115cc5618bc0e71a05111d105` |
| render commit | `c0e37f94c74d2c695df1ca5105400a2976ea18be` (from the hrefs) |
| verso-blueprint | `v4.33.0` (`vbpInternalSchemaVersion` 8) |
| raw | 13.6 MB, 364 previews, 10 graphs (identical copies of the unified graph, one per chapter page) |
| projected | 212 KB, 147 previews (those a `previewKey` names), 1 graph, 147 nodes |

`project.jq` keeps one graph and fails if the 10 copies ever project
differently, so a future render with distinct per-chapter graphs is not silently
truncated. The adapter's in-manifest label de-duplication is covered by a unit
test, not by this fixture.

## Regenerate and verify

```bash
curl -sSL -o /tmp/sm-manifest.json \
  https://beneficial-ai-foundation.github.io/secure-messaging/-verso-data/blueprint-manifest.json
sha256sum /tmp/sm-manifest.json   # compare with the table above
jq -c -f project.jq /tmp/sm-manifest.json > blueprint-manifest.json
```

The projection is not a pure field filter: `.definedDefs // []` rewrites a
missing or `null` list to `[]` and `map({name})` assumes a `name` key. So after
regenerating, check that the adapter produces the same output from the raw
download and from the projection:

```bash
# from the repository root; -e on jq makes a missing/empty output fail
set -e
F=tests/fixtures/verso/secure-messaging-schema8/blueprint-manifest.json
for m in raw:/tmp/sm-manifest.json proj:$F; do
  n=${m%%:*}; p=${m#*:}
  cargo run -q -- extract . --adapter verso --no-render --verso-manifest "$p" \
    --lean tests/fixtures/lean/secure-messaging-atoms.json \
    -o /tmp/$n.json --summary-output /tmp/$n-summary.json 2>/dev/null
  jq -S -e .data /tmp/$n.json > /tmp/$n-data.json
  jq -S -e .data /tmp/$n-summary.json > /tmp/$n-sum.json
done
diff /tmp/raw-data.json /tmp/proj-data.json && diff /tmp/raw-sum.json /tmp/proj-sum.json && echo IDENTICAL
```

Passed on 2026-09-17: both `.data` payloads byte-identical. On this manifest
nothing was rewritten (all 279 `literateDeclarations` are already empty).

The committed atom base `tests/fixtures/lean/secure-messaging-atoms.json` is
from commit `6a4fce0`, not `c0e37f9`, so this fixture is not joined with it in
tests; the check above uses it only as a shared input.
