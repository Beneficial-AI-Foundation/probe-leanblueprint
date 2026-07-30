#!/usr/bin/env bash
#
# Reproduce a "real Verso blueprint" example end-to-end, exactly as verilib
# would: the ONLY input is the upstream Lean repo. This script runs the full
# pipeline itself — build, render the blueprint, run probe-lean for the atom
# base, then join with probe-leanblueprint.
#
# It is intentionally generic; each examples/<name>/README.md invokes it with
# that project's parameters.
#
# Requirements on PATH:
#   - lake / elan (toolchains are auto-fetched)
#   - probe-lean-<toolchain>   (a probe-lean built for the project's Lean version)
#   - probe-leanblueprint
#   - python3 (for scripts/blueprint_stats.py)
#
# Usage:
#   reproduce-verso-example.sh \
#     --repo   https://github.com/ejgallego/verso-sphere-packing \
#     --render SpherePackingBlueprintMain.lean \
#     --sub    Sphere-Packing-LaTeX-Reference \
#     --lib    SpherePacking \
#     --probe-lean probe-lean-v4.31.0 \
#     [--skip-lfs] \
#     [--workdir /tmp/verso-real]
#
set -euo pipefail

REPO="" RENDER="" SUB="" LIB="" PROBE_LEAN="probe-lean" SKIP_LFS=0
WORKDIR="${WORKDIR:-$(pwd)/_verso-real}"
while [ $# -gt 0 ]; do
  case "$1" in
    --repo)        REPO="$2"; shift 2 ;;
    --render)      RENDER="$2"; shift 2 ;;
    --sub)         SUB="$2"; shift 2 ;;
    --lib)         LIB="$2"; shift 2 ;;
    --probe-lean)  PROBE_LEAN="$2"; shift 2 ;;
    --workdir)     WORKDIR="$2"; shift 2 ;;
    --skip-lfs)    SKIP_LFS=1; shift ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done
[ -n "$REPO" ] && [ -n "$RENDER" ] && [ -n "$SUB" ] && [ -n "$LIB" ] || {
  echo "missing required args (see header)" >&2; exit 2; }

name="$(basename "$REPO")"
mkdir -p "$WORKDIR"; cd "$WORKDIR"

# 1. Clone the upstream blueprint repo (+ submodules: the math lib + verso harness).
if [ ! -d "$name" ]; then
  if [ "$SKIP_LFS" = 1 ]; then
    GIT_LFS_SKIP_SMUDGE=1 git clone --recurse-submodules --depth 1 --shallow-submodules "$REPO.git" "$name"
  else
    git clone --recurse-submodules --depth 1 --shallow-submodules "$REPO.git" "$name"
  fi
fi
cd "$name"

# 2. Mathlib cache + build the blueprint overlay (this also builds the math lib,
#    which is a local path dependency, into $SUB/.lake/build).
lake exe cache get
lake build

# 3. Render the blueprint -> _out/site/.../-verso-data/blueprint-manifest.json
lake env lean --run "$RENDER" --output _out/site
MANIFEST="$(find _out -name blueprint-manifest.json | head -1)"
echo "manifest: $MANIFEST"

# 4. Atom base: run probe-lean on the MATH submodule (the actual formalization),
#    restricted to its library. Its oleans are already built from step 2.
( cd "$SUB" && "$PROBE_LEAN" extract . -l "$LIB" -o "$WORKDIR/$name.atoms.json" )

# 5. Join: enrich the atoms with blueprint status from the rendered manifest.
probe-leanblueprint extract . \
  --adapter verso \
  --lean "$WORKDIR/$name.atoms.json" \
  --verso-manifest "$MANIFEST" \
  -o "$WORKDIR/$name.extract.json" \
  --summary-output "$WORKDIR/$name.summary.json"

# 6. Human-readable report.
python3 "$(dirname "$0")/../scripts/blueprint_stats.py" "$WORKDIR/$name.extract.json"
