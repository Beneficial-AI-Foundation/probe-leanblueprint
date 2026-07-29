#!/usr/bin/env python3
"""Display Lean blueprint progress stats from a probe-leanblueprint extract JSON.

Reads a `probe-leanblueprint/extract` envelope (the enriched atoms) and prints a
readable two-axis progress report: headline, overall statement/proof tables, a
per-chapter breakdown, and any status mismatches / missing declarations.

This recomputes everything from the extract's `blueprint-*` extension fields, so
it doubles as an independent cross-check of the tool's summary sidecar.

Usage:
    python3 blueprint_stats.py <extract.json>
    python3 blueprint_stats.py <extract.json> --json     # machine-readable
"""
import argparse
import json
import sys
from collections import defaultdict

STATEMENT_ORDER = ["formalized", "ready", "blocked", "none"]
PROOF_ORDER = ["fully-proved", "proved", "ready", "none"]


class Node:
    __slots__ = (
        "label", "kind", "chapter", "statement", "proof",
        "bound", "decl_missing", "decl_upstream_proved", "missing_decls", "mismatch",
    )

    def __init__(self, label):
        self.label = label
        self.kind = "theorem"
        self.chapter = "ungrouped"
        self.statement = "none"
        self.proof = "none"
        self.bound = False
        self.decl_missing = False
        self.decl_upstream_proved = False
        self.missing_decls = []
        self.mismatch = None


def collect_nodes(data):
    """Group atoms by blueprint-label into one record per blueprint node."""
    nodes = {}
    for atom in data.values():
        label = atom.get("blueprint-label")
        if label is None:
            continue
        n = nodes.get(label)
        if n is None:
            n = Node(label)
            nodes[label] = n
        n.kind = atom.get("blueprint-kind", n.kind)
        n.chapter = atom.get("blueprint-chapter", n.chapter)
        n.statement = atom.get("blueprint-statement-status", n.statement)
        n.proof = atom.get("blueprint-proof-status", n.proof)
        # A node is "bound" if any of its atoms is a real (non-synthetic) atom,
        # or a shadow atom (a genuinely-bound node whose Lean atom was claimed by
        # a colliding node, preserved synthetically to keep the extract
        # node-complete).
        if atom.get("language") != "blueprint" or atom.get("blueprint-shadow"):
            n.bound = True
        if atom.get("blueprint-decl-missing"):
            n.decl_missing = True
        if atom.get("blueprint-decl-upstream-proved"):
            n.decl_upstream_proved = True
        missing = atom.get("blueprint-missing-decls")
        if missing:
            n.missing_decls = sorted(set(n.missing_decls) | set(missing))
        if atom.get("blueprint-status-mismatch"):
            n.mismatch = atom["blueprint-status-mismatch"]
    return list(nodes.values())


def axis_counts(nodes, axis):
    by_kind = {"all": defaultdict(int), "definition": defaultdict(int), "theorem": defaultdict(int)}
    for n in nodes:
        status = getattr(n, axis)
        by_kind["all"][status] += 1
        bucket = "definition" if n.kind == "definition" else "theorem"
        by_kind[bucket][status] += 1
    return by_kind


def fmt_table(title, axis_label, counts, order):
    lines = [f"{title}"]
    lines.append(f"  {axis_label:<16}{'all':>6}{'def':>6}{'thm':>6}")
    for status in order:
        a = counts["all"].get(status, 0)
        d = counts["definition"].get(status, 0)
        t = counts["theorem"].get(status, 0)
        lines.append(f"  {status:<16}{a:>6}{d:>6}{t:>6}")
    return "\n".join(lines)


def build_report(env):
    data = env.get("data", {})
    nodes = collect_nodes(data)

    total = len(nodes)
    bound = sum(1 for n in nodes if n.bound)
    decl_missing = sum(1 for n in nodes if n.decl_missing and not n.bound)
    decl_missing_upstream = sum(
        1 for n in nodes if n.decl_missing and not n.bound and n.decl_upstream_proved
    )
    planned = sum(1 for n in nodes if not n.bound and not n.decl_missing)
    partial_missing = [n for n in nodes if n.missing_decls]
    mismatches = [n for n in nodes if n.mismatch]

    thms = [n for n in nodes if n.kind != "definition"]
    thm_total = len(thms)
    thm_proved = sum(1 for n in thms if n.proof == "fully-proved")
    # probe-lean-confirmed: the blueprint claims fully-proved, the node's whole
    # binding is present (bound with no missing decls), and probe-lean did not
    # contradict it (no blueprint-status-mismatch). This is a "not refuted" bar,
    # not "affirmatively verified" — an unbound/decl-missing/partial-missing
    # fully-proved node is dropped, but a fully-present one with no/`trusted`/
    # locally-`verified` status still counts. Mirrors the Rust summary Headline
    # (`missing.is_empty()` gate included); see P26.
    thm_proved_confirmed = sum(
        1 for n in thms
        if n.proof == "fully-proved" and n.bound and not n.missing_decls and not n.mismatch
    )
    # Upstream-proved: fully-proved theorems that are decl-missing here but proved
    # out-of-workspace (a dependency, commonly Mathlib/stdlib) per the Verso
    # renderer. Neither a local confirmation nor a gap. Always 0 for Massot.
    thm_upstream_proved = sum(
        1 for n in thms
        if n.proof == "fully-proved" and n.decl_missing and not n.bound and n.decl_upstream_proved
    )
    fraction = (thm_proved / thm_total) if thm_total else 0.0
    fraction_confirmed = (thm_proved_confirmed / thm_total) if thm_total else 0.0

    by_chapter = defaultdict(lambda: {"nodes": 0, "stmt_formalized": 0, "thm_total": 0, "thm_proved": 0})
    for n in nodes:
        c = by_chapter[n.chapter]
        c["nodes"] += 1
        if n.statement == "formalized":
            c["stmt_formalized"] += 1
        if n.kind != "definition":
            c["thm_total"] += 1
            if n.proof == "fully-proved":
                c["thm_proved"] += 1

    return {
        "nodes": nodes,
        "totals": {
            "nodes": total, "bound": bound, "planned-only": planned,
            "decl-missing": decl_missing,
            "decl-missing-upstream-proved": decl_missing_upstream,
            "partial-missing": len(partial_missing),
            "mismatches": len(mismatches),
        },
        "headline": {
            "theorems-total": thm_total, "theorems-fully-proved": thm_proved,
            "theorems-fully-proved-probe-lean-confirmed": thm_proved_confirmed,
            "theorems-fully-proved-upstream-proved": thm_upstream_proved,
            "fraction": fraction, "fraction-probe-lean-confirmed": fraction_confirmed,
        },
        "statement": axis_counts(nodes, "statement"),
        "proof": axis_counts(nodes, "proof"),
        "by-chapter": dict(sorted(by_chapter.items())),
        "mismatch-list": [(n.label, n.mismatch) for n in mismatches],
        "decl-missing-list": [
            (n.label, n.decl_upstream_proved)
            for n in nodes if n.decl_missing and not n.bound
        ],
        "partial-missing-list": [(n.label, n.missing_decls) for n in partial_missing],
    }


def print_report(env, report):
    tool = env.get("tool", {})
    src = env.get("source", {})
    pkg = src.get("package", "?")
    ver = src.get("package-version", "")
    h = report["headline"]
    t = report["totals"]

    print(f"probe-leanblueprint stats — {pkg} {ver}".rstrip())
    if src.get("repo"):
        print(f"  source: {src['repo']}@{src.get('commit', '')[:10]}")
    print()
    confirmed = h["theorems-fully-proved-probe-lean-confirmed"]
    claimed = h["theorems-fully-proved"]
    upstream = h.get("theorems-fully-proved-upstream-proved", 0)
    pct = h["fraction-probe-lean-confirmed"] * 100
    upstream_suffix = f" (+{upstream} upstream-proved)" if upstream else ""
    print(f"Headline: {confirmed}/{h['theorems-total']} theorems probe-lean-confirmed fully proved ({pct:.1f}%){upstream_suffix}")
    unbacked = claimed - confirmed - upstream
    if unbacked > 0:
        print(f"  (blueprint claims {claimed}/{h['theorems-total']}; "
              f"{unbacked} not backed by probe-lean's verification status)")
    dm_upstream = t.get("decl-missing-upstream-proved", 0)
    decl_missing = (
        f"decl-missing {t['decl-missing']} ({dm_upstream} upstream-proved)"
        if dm_upstream else f"decl-missing {t['decl-missing']}"
    )
    print(
        f"Blueprint nodes: {t['nodes']}   "
        f"(bound {t['bound']} · planned-only {t['planned-only']} · "
        f"{decl_missing} · partial-missing {t['partial-missing']} · "
        f"mismatches {t['mismatches']})"
    )
    print()
    print(fmt_table("Statement axis (is the statement formalized?)", "status", report["statement"], STATEMENT_ORDER))
    print()
    print(fmt_table("Proof axis (is the proof complete?)", "status", report["proof"], PROOF_ORDER))
    print()

    print(f"By chapter{'':<26}{'nodes':>7}{'stmt✓':>7}{'thm✓/thm':>10}")
    for chapter, c in report["by-chapter"].items():
        name = chapter if len(chapter) <= 34 else chapter[:31] + "..."
        ratio = f"{c['thm_proved']}/{c['thm_total']}"
        print(f"  {name:<34}{c['nodes']:>7}{c['stmt_formalized']:>7}{ratio:>10}")
    print()

    if report["mismatch-list"]:
        print("Status mismatches (blueprint over-claims vs machine verification):")
        for label, reason in report["mismatch-list"]:
            print(f"  ! {label}: {reason}")
    else:
        print("Status mismatches: none")

    if report["decl-missing-list"]:
        print("Declarations bound in blueprint but missing from probe-lean:")
        for label, upstream in report["decl-missing-list"]:
            note = "  (proved out-of-workspace)" if upstream else ""
            print(f"  ? {label}{note}")
    else:
        print("Missing declarations: none")

    if report["partial-missing-list"]:
        print("Bound nodes with some declarations missing from probe-lean:")
        for label, decls in report["partial-missing-list"]:
            print(f"  ? {label}: {', '.join(decls)}")
    else:
        print("Partial-missing declarations: none")


def main(argv):
    ap = argparse.ArgumentParser(description="Display probe-leanblueprint progress stats.")
    ap.add_argument("extract", help="path to a probe-leanblueprint/extract JSON")
    ap.add_argument("--json", action="store_true", help="emit machine-readable JSON instead of a report")
    args = ap.parse_args(argv)

    with open(args.extract) as f:
        env = json.load(f)

    schema = env.get("schema", "")
    if schema != "probe-leanblueprint/extract":
        sys.stderr.write(
            f"error: expected schema 'probe-leanblueprint/extract', got '{schema}'; "
            "this script reads probe-leanblueprint's own enriched extract\n"
        )
        return 2

    report = build_report(env)
    if args.json:
        report = {k: v for k, v in report.items() if k != "nodes"}
        json.dump(report, sys.stdout, indent=2, sort_keys=True)
        print()
    else:
        print_report(env, report)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
