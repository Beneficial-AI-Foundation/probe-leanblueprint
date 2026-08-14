"""Headless leanblueprint extractor.

Parses a Patrick Massot `leanblueprint` LaTeX source (typically
`blueprint/src/web.tex`) with plasTeX, reusing leanblueprint's own parser and
status computation, and dumps normalized node/edge data to JSON on stdout.

No HTML is rendered and no Lean build is required: plasTeX only parses LaTeX,
and leanblueprint's post-parse callbacks compute the per-node statement/proof
status that we serialize here.

Requires: plasTeX, plastexdepgraph, leanblueprint (and graphviz/libgraphviz-dev
for pygraphviz). Install with `pip install leanblueprint`.

Usage:
    python3 blueprint_emit.py path/to/web.tex
"""
import json
import sys


def _item_kind(node):
    from plastexdepgraph.Packages.depgraph import item_kind
    return item_kind(node)


def _chapter_title(node):
    """Title of the outermost sectioning ancestor (chapter, or the top-level
    division the document actually uses), or None. Graph nodes are plasTeX DOM
    elements, so the paper position is recovered by walking `parentNode` and
    keeping the last ancestor whose `level` is a real sectioning level
    (plasTeX: part=-1 ... subsubsection=3; `document` is a huge negative
    sentinel and paragraphs sit at 100+)."""
    title = None
    current = getattr(node, "parentNode", None)
    while current is not None:
        level = getattr(current, "level", None)
        if isinstance(level, int) and -2 <= level <= 3:
            candidate = getattr(current, "title", None)
            text = getattr(candidate, "textContent", candidate)
            if isinstance(text, str) and text.strip():
                title = text.strip()
        current = getattr(current, "parentNode", None)
    return title


def _label_anchors():
    """Map every `\\label{...}` in the blueprint source tree (cwd) to its
    `(path, 1-based line)` — node ids are exactly these labels, and plasTeX
    does not retain per-environment positions, so the anchor is recovered from
    the source text. First occurrence wins (duplicate labels are a blueprint
    bug plasTeX warns about separately)."""
    import os
    import re

    anchors = {}
    pattern = re.compile(r"\\label\s*\{([^}]*)\}")
    # An unescaped % starts a TeX comment: strip it so `% \label{x}` in a
    # comment can never beat the real declaration site.
    comment = re.compile(r"(?<!\\)%.*")
    for root, dirs, files in os.walk("."):
        dirs.sort()  # deterministic traversal, so first-wins is stable
        for name in sorted(files):
            if not name.endswith(".tex"):
                continue
            path = os.path.normpath(os.path.join(root, name))
            try:
                with open(path, encoding="utf-8", errors="replace") as fh:
                    for lineno, line in enumerate(fh, start=1):
                        for match in pattern.finditer(comment.sub("", line)):
                            anchors.setdefault(match.group(1), (path, lineno))
            except OSError:
                continue
    return anchors


def _statement_source(node, limit=10000):
    """plasTeX's reconstructed LaTeX of the environment (statement content),
    capped defensively."""
    try:
        text = node.source
    except Exception:
        return None
    if not isinstance(text, str) or not text.strip():
        return None
    return text.strip()[:limit]


def extract(path):
    import os

    from plasTeX.Config import defaultConfig
    from plasTeX.Compile import parse

    config = defaultConfig()
    # Activate the plugins so `\usepackage{blueprint}` resolves and its
    # post-parse callbacks run.
    config["general"]["plugins"] = ["plastexdepgraph", "leanblueprint"]
    config["files"]["log"] = False

    # plasTeX resolves the input (and any `\input`) via kpsewhich relative to the
    # working directory, and leanblueprint writes its `lean_decls` next to it, so
    # run from the file's directory and pass the bare filename.
    directory, filename = os.path.split(os.path.abspath(path))
    if directory:
        os.chdir(directory)
    tex = parse(filename, config)
    document = tex.ownerDocument

    graphs = document.userdata.get("dep_graph", {}).get("graphs", {})
    anchors = _label_anchors()
    nodes = {}
    edges = []
    for _section, graph in graphs.items():
        for node in graph.nodes:
            label = node.id
            if label in nodes:
                continue
            anchor = anchors.get(label)
            data = node.userdata
            nodes[label] = {
                "label": label,
                "kind": _item_kind(node),
                "lean_decls": list(data.get("leandecls", [])),
                "leanok": bool(data.get("leanok", False)),
                "mathlibok": bool(data.get("mathlibok", False)),
                "notready": bool(data.get("notready", False)),
                "can_state": bool(data.get("can_state", False)),
                "can_prove": bool(data.get("can_prove", False)),
                "proved": bool(data.get("proved", False)),
                "fully_proved": bool(data.get("fully_proved", False)),
                "issue": data.get("issue"),
                "chapter": _chapter_title(node),
                "source_path": anchor[0] if anchor else None,
                "source_line": anchor[1] if anchor else None,
                "source": _statement_source(node),
            }
        for s, t in graph.edges:
            edges.append({"source": s.id, "target": t.id, "axis": "statement"})
        for s, t in graph.proof_edges:
            edges.append({"source": s.id, "target": t.id, "axis": "proof"})

    return {"nodes": list(nodes.values()), "edges": edges}


def main(argv):
    if len(argv) != 2:
        sys.stderr.write("usage: blueprint_emit.py path/to/web.tex\n")
        return 2
    json.dump(extract(argv[1]), sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
