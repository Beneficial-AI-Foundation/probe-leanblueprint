#!/usr/bin/env bash
# Look up blueprint <-> Lean links in the probe-leanblueprint extract and Verso manifest.
#
# Usage:
#   blueprint-lookup.sh meta  <LeanName>   blueprint metadata of a Lean declaration
#   blueprint-lookup.sh code  <LeanName>   file:lines of a Lean declaration
#   blueprint-lookup.sh href  <LeanName>   rendered-doc href for its blueprint node
#   blueprint-lookup.sh decls <label>      Lean declaration(s) bound to a blueprint node
#   blueprint-lookup.sh src   <label>      authored Verso source of the informal statement
#   blueprint-lookup.sh table              full label <-> Lean-name mapping
#
# Names are unprefixed (EtM.etmAEAD_security, not probe:...). Run it from
# anywhere inside the target Lean project (it resolves the project's git root);
# needs a prior `probe-leanblueprint extract` and `lake exe vbp build`.
#
# Stopgap for the native `query` subcommand (issue #28); delete when that lands.
set -euo pipefail

root=$(git rev-parse --show-toplevel)
envelope=$(ls -t "$root"/.verilib/probes/leanblueprint_*.json 2>/dev/null | grep -v _summary | head -1) \
  || { echo "no extract envelope under .verilib/probes/ (run probe-leanblueprint extract)" >&2; exit 1; }
manifest="$root/_out/site/html-multi/-verso-data/blueprint-manifest.json"

cmd=${1:?usage: blueprint-lookup.sh meta|code|href|decls|src|table [name-or-label]}
arg=${2:-}
[[ $cmd == table || -n $arg ]] || { echo "missing name/label argument" >&2; exit 1; }

need_manifest() {
  [[ -f $manifest ]] || { echo "no manifest at $manifest (run: lake exe vbp build)" >&2; exit 1; }
}

case $cmd in
  meta)
    jq --arg n "$arg" '.data["probe:\($n)"]
      | if . == null then "not in extract"
        else with_entries(select(.key|startswith("blueprint"))) end' "$envelope" ;;
  code)
    jq -r --arg n "$arg" '.data["probe:\($n)"]
      | if . == null then "not in extract"
        else "\(."code-path"):\(."code-text"."lines-start")-\(."code-text"."lines-end")" end' "$envelope" ;;
  href)
    need_manifest
    label=$(jq -r --arg n "$arg" '.data["probe:\($n)"]["blueprint-label"] // empty' "$envelope")
    [[ -n $label ]] || { echo "no blueprint node references $arg" >&2; exit 1; }
    jq -r --arg l "$label" '[.graphs[].nodes[] | select(.label==$l).href] | unique[]' "$manifest" ;;
  decls)
    jq -r --arg l "$arg" '.data["probe:blueprint:\($l)"].dependencies // ["(no node atom for that label)"] | .[]' "$envelope" ;;
  src)
    need_manifest
    jq -r --arg l "$arg" 'first(.previews[] | select(.label==$l)) // "no preview for that label"
      | if type == "string" then .
        else .sourceLocation.location | "\(.path):\(.range.start.line+1)" end' "$manifest" ;;
  table)
    jq -r '.data | to_entries[]
      | select(.value["blueprint-label"] and (.key|startswith("probe:blueprint:")|not))
      | "\(.value["blueprint-label"])\t\(.key | ltrimstr("probe:"))"' "$envelope" | sort ;;
  *)
    echo "unknown command: $cmd (meta|code|href|decls|src|table)" >&2; exit 1 ;;
esac
