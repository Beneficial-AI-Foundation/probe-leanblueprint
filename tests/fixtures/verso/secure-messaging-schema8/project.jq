# Projection of a verso-blueprint unified manifest (vbpInternalSchemaVersion 8)
# to the fields the probe-leanblueprint Verso adapter reads. See README.md for
# the source, the equivalence check and how to regenerate:
#
#   jq -c -f project.jq blueprint-manifest.full.json > blueprint-manifest.json
#
# Kept: the schema marker; one graph, with node objects reduced to the fields
# `Node` deserializes (use edges reduced to `label`) and `edges`/`groups` whole;
# `sourceDocuments` whole; only the previews some node's `previewKey` names,
# each reduced to `key`, `facet`, `tags`, `sourceLocation` and `codeData` with
# `canonical`/`present`/`provedStatus`/`provenance` per declaration. Dropped:
# rendered HTML, hover payloads, graph `variants` (DOT sources), node `visual`.
#
# The unified manifest repeats the same graph once per rendered chapter page
# (10 copies on 2026-09-17). Only the first is kept, guarded: regeneration fails
# if the projected graphs ever differ, so a future render with distinct
# per-chapter graphs is not silently truncated.
def node_fields:
  { label, kind, parent, title, href, previewKey, statementStatus, proofStatus,
    statementUses: (.statementUses | map({label})),
    proofUses: (.proofUses | map({label})) };
def graph_fields:
  { schemaVersion, edges, groups, nodes: (.nodes | map(node_fields)) };
def decl_fields:
  { canonical, present, provedStatus, provenance };
def code_data:
  if . == null then null else
    with_entries(select(.key == "external" or .key == "inline"
                        or .key == "externalDecls" or .key == "literateDeclarations"))
    | if has("externalDecls") then .externalDecls |= map(decl_fields) else . end
    | if has("literateDeclarations") then
        .literateDeclarations |= { definedDefs: (.definedDefs // [] | map({name})),
                                   definedTheorems: (.definedTheorems // [] | map({name})) }
      else . end
  end;
([.graphs[] | graph_fields | tojson] | unique) as $distinct
| if ($distinct | length) != 1 then
    error("expected every graph to project identically; got \($distinct | length) distinct graphs out of \(.graphs | length)")
  else . end
| ([.graphs[].nodes[].previewKey | select(. != null)] | unique) as $keys
| { vbpInternalSchemaVersion,
    graphs: [ .graphs[0] | { key } + graph_fields ],
    sourceDocuments,
    previews: [ .previews[] | select(.key as $k | $keys | index($k))
                | { key, facet, tags, sourceLocation, codeData: (.codeData | code_data) } ] }
