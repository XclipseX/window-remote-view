---
name: graphify
description: Turn a codebase into a knowledge graph (modules, classes, functions, calls, imports) so the agent can understand structure without re-reading every file. Use when starting work on an unfamiliar repo, before big refactors, when asked "how does X connect to Y", or when context is running low and you need a compact map of the code.
---

# Graphify – Code Knowledge Graph

Goal: build a **compact, queryable map** of the codebase once, then read the map instead of re-reading files. This saves tokens and keeps the agent's picture of the architecture consistent across sessions.

## When to use
- First time opening a repo / onboarding.
- Before refactoring, renaming, or deleting something (find every dependent).
- Answering "where is X used", "what calls Y", "which modules are core".
- When the graph is older than the latest big change → rebuild.

## Step 1 – Build the graph

### Option A: `graphify` (preferred – tested with graphifyy 0.9.x, offline, no API key)
```bash
pip install graphifyy          # PyPI name has double "y"; CLI is `graphify`
graphify update .              # AST extraction + clustering, no LLM needed
```
Output in `graphify-out/`: `graph.json`, `GRAPH_REPORT.md` (hubs, communities), `graph.html` (interactive view).
Query instead of reading files:
```bash
graphify god-nodes --top 10                 # architectural hubs
graphify query "how is auth handled" --budget 1500
graphify explain "UserService"              # a node and its neighbours
graphify affected "parse_config" --depth 2  # what breaks if this changes (blast radius)
graphify path "A" "B"                       # how two things connect
```
Notes: files without a supported extractor (CSV, data, LICENSE…) are skipped and reported – the graph covers code only.
Optional: `graphify install --platform claude|codex|gemini|cursor|agents` adds graphify's own agent skill; `/graphify` inside the agent also extracts docs/images (that part uses an LLM). `graphify watch .` rebuilds on change.

### Option B: built-in fallback (no install, Python 3 only)
```bash
python3 tools/skillpack/code_graph.py <repo_path> --out .graph
```
Produces `.graph/graph.json` and `.graph/GRAPH.md` (file → imports / definitions, most-imported "hub" files). Regex heuristics for Python, JS/TS, Go, Rust, Java/Kotlin, PHP, C#, Ruby – file-level imports only (no call graph). Use it when pip is unavailable.

## Step 2 – Read the map, not the code
1. Read `GRAPH_REPORT.md` / `GRAPH.md` first. Note the **hub** nodes (most connections) – these are the core of the system.
2. For a question, find the relevant nodes in `graph.json` (grep by name), follow edges 1–2 hops, and only then open the specific files.
3. Before changing a symbol, list every incoming edge (`imported_by` / callers) – that is your blast radius.

## Step 3 – Keep it fresh
- Rebuild with `graphify update .` (incremental, cached) after: adding/removing modules, large refactors, or when results disagree with the code.
- Add `graphify-out/` or `.graph/` to `.gitignore` unless the team wants it versioned.
- Pair with the **obsidian-memory** skill: write architecture decisions as notes that link to graph nodes.

## Rules
- Never trust the graph over the code when they conflict – the code wins, then rebuild.
- Don't paste the whole `graph.json` into context; query it (grep / jq / small script).
- Report to the user: number of nodes/edges, top hubs, and anything surprising (cycles, god-files).
