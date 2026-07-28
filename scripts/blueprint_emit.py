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
    nodes = {}
    edges = []
    for _section, graph in graphs.items():
        for node in graph.nodes:
            label = node.id
            if label in nodes:
                continue
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
                "has_proof": bool(data.get("proved_by")),
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
