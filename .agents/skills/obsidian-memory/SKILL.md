---
name: obsidian-memory
description: Use an Obsidian-style Markdown vault (folder of .md notes with [[wikilinks]], tags and YAML frontmatter) as long-term project memory for AI coding agents. Use at session start to recall context, after finishing a task to record what changed and why, when making architecture decisions, or when the user says "remember", "note this", "what did we decide".
---

# Obsidian Memory – Persistent Project Brain

The agent forgets everything between sessions. A vault of linked Markdown notes fixes that: plain files, readable by any agent (Claude, Gemini, ChatGPT/Codex) and by humans in Obsidian.

## Vault location
Default: `docs/vault/` inside the repo (versioned with code). Alternative: the user's existing Obsidian vault – ask for the path once and reuse it.

Create if missing:
```bash
bash tools/skillpack/init_vault.sh docs/vault
```

## Structure
```
vault/
  00-Index.md          # Map of Content: links to everything important. Read FIRST.
  architecture/        # one note per module/subsystem
  decisions/           # ADRs: ADR-0001-use-postgres.md
  sessions/            # daily/session logs: 2026-11-23.md
  bugs/                # bug + root cause + fix
  snippets/            # reusable code patterns
```
Templates live in `templates/` next to this file.

## Workflow
**Session start (recall)**
1. Read `00-Index.md`, then the latest 1–2 files in `sessions/`.
2. Grep the vault for keywords of the current task: `grep -ril "<keyword>" docs/vault`.
3. Follow `[[links]]` only as far as needed.

**During work** – when you decide something non-obvious, create a note in `decisions/` immediately.

**Session end (store)**
1. Append to `sessions/YYYY-MM-DD.md`: goal, what changed (files), why, open TODOs.
2. Update/create the `architecture/` note of each module you touched.
3. Add new notes to `00-Index.md`.
4. In a cloud session: commit + push the vault (see **cloud-session-persistence**) – otherwise the notes are lost.

## Note rules
- Frontmatter on every note:
  ```yaml
  ---
  type: architecture | decision | session | bug | snippet
  tags: [auth, backend]
  related: ["[[auth-service]]"]
  updated: 2026-11-23
  ---
  ```
- One idea per note; filenames kebab-case; link generously with `[[note-name]]`.
- Reference code as `path/to/file.py:42`, never paste large code blocks (code lives in the repo).
- Keep notes short (< 150 lines). Split when longer.
- Never store secrets, tokens, or personal data.

## With graphify
Run the **graphify** skill to get the structural map; store the *why* (decisions, gotchas) in the vault. Graph = what the code is, vault = why it is that way. If graphify supports `--obsidian`, export its nodes into `vault/graph/` and link them from architecture notes.
