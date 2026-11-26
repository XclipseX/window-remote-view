#!/usr/bin/env python3
"""Minimal, dependency-free code graph builder (fallback for graphify).

Usage: python3 code_graph.py <repo> [--out .graph]
Writes graph.json (nodes/edges) and GRAPH.md (summary for the agent).
"""
import argparse, json, os, re
from collections import Counter, defaultdict

SKIP = {".git", ".cache", ".mypy_cache", ".pytest_cache", ".tox", ".idea", ".vscode", "node_modules", "dist", "build", ".venv", "venv", "__pycache__",
        ".next", "target", ".graph", "graphify-out", "vendor"}
EXT = {".py", ".js", ".jsx", ".ts", ".tsx", ".go", ".rs", ".java", ".kt", ".rb", ".php", ".cs"}
PY = re.compile(r"^\s*(?:from\s+([\w.]+)\s+import|import\s+([\w.]+))", re.M)
JS = re.compile(r"""(?:^\s*import\s[^'"]*?from\s*|^\s*import\s*|require\(\s*|import\(\s*)['"]([^'"]+)['"]""", re.M)
GO = re.compile(r'^\s*(?:import\s+)?(?:\w+\s+)?"([\w./-]+)"\s*$', re.M)  # only meaningful inside import blocks
USE = re.compile(r"^\s*use\s+([\w:\\]+)", re.M)
JAVA = re.compile(r"^\s*import\s+(?:static\s+)?([\w.]+)", re.M)
IMPORT_RE_BY_EXT = {".py": [PY], ".js": [JS], ".jsx": [JS], ".ts": [JS], ".tsx": [JS], ".go": [GO],
                    ".rs": [USE], ".php": [USE], ".java": [JAVA], ".kt": [JAVA], ".cs": [USE],
                    ".rb": [re.compile(r"""^\s*require(?:_relative)?\s+['"]([^'"]+)['"]""", re.M)]}
DEF_RE = re.compile(
    r"^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?(?:pub\s+)?"
    r"(?:def|class|function|func|fn|interface|struct|enum|type)\s+(\w+)", re.M)


def resolve(imp, src, files_by_stem):
    if imp.startswith("."):
        base = os.path.normpath(os.path.join(os.path.dirname(src), imp))
        for f in files_by_stem.get(os.path.basename(base), []):
            if os.path.splitext(f)[0] in (base, os.path.join(base, "index")) or f.startswith(base):
                return f
    key = imp.replace("::", ".").replace("/", ".").split(".")[-1]
    cands = files_by_stem.get(key, [])
    return cands[0] if len(cands) == 1 else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("repo")
    ap.add_argument("--out", default=".graph")
    a = ap.parse_args()
    root = os.path.abspath(a.repo)
    files = []
    for d, dirs, fs in os.walk(root):
        dirs[:] = [x for x in dirs if x not in SKIP]
        files += [os.path.relpath(os.path.join(d, f), root) for f in fs if os.path.splitext(f)[1] in EXT]
    by_stem = defaultdict(list)
    for f in files:
        stem = os.path.splitext(os.path.basename(f))[0]
        by_stem[stem if stem not in ("index", "__init__") else os.path.basename(os.path.dirname(f))].append(f)

    nodes, edges, external = [], [], Counter()
    for f in files:
        try:
            text = open(os.path.join(root, f), encoding="utf-8", errors="ignore").read()
        except OSError:
            continue
        defs = sorted(set(DEF_RE.findall(text)))
        nodes.append({"id": f, "kind": "file", "lines": text.count("\n") + 1, "defines": defs})
        seen = set()
        for rx in IMPORT_RE_BY_EXT.get(os.path.splitext(f)[1], []):
            for m in rx.finditer(text):
                imp = next(g for g in m.groups() if g)
                tgt = resolve(imp, f, by_stem)
                if tgt and tgt != f and tgt not in seen:
                    seen.add(tgt); edges.append({"from": f, "to": tgt, "kind": "imports"})
                elif not tgt:
                    external[imp.split("/")[0].split(".")[0]] += 1

    indeg = Counter(e["to"] for e in edges)
    outdeg = Counter(e["from"] for e in edges)
    os.makedirs(a.out, exist_ok=True)
    json.dump({"root": root, "nodes": nodes, "edges": edges}, open(os.path.join(a.out, "graph.json"), "w"), indent=1)

    by_id = {n["id"]: n for n in nodes}
    imported_by = defaultdict(list)
    for e in edges:
        imported_by[e["to"]].append(e["from"])
    md = [f"# Code Graph: {os.path.basename(root)}", "",
          f"- Files: {len(nodes)}  |  Internal import edges: {len(edges)}", "",
          "## Hubs (most imported = core, change carefully)"]
    md += [f"- `{f}` ← {c} files" for f, c in indeg.most_common(15)] or ["- (none)"]
    md += ["", "## Heavy importers (orchestrators / entry points)"]
    md += [f"- `{f}` → {c} files" for f, c in outdeg.most_common(10)] or ["- (none)"]
    md += ["", "## Top external dependencies"]
    md += [f"- {k} ({v})" for k, v in external.most_common(15)]
    md += ["", "## Files"]
    for f in sorted(by_id):
        n = by_id[f]
        defs = ", ".join(n["defines"][:12]) + (" …" if len(n["defines"]) > 12 else "")
        md.append(f"- `{f}` ({n['lines']} lines){' — ' + defs if defs else ''}"
                  + (f"  ⟵ {len(imported_by[f])}" if imported_by[f] else ""))
    open(os.path.join(a.out, "GRAPH.md"), "w").write("\n".join(md) + "\n")
    print(f"Wrote {a.out}/graph.json and {a.out}/GRAPH.md ({len(nodes)} files, {len(edges)} edges)")


if __name__ == "__main__":
    main()
