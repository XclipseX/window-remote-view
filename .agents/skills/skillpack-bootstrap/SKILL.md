---
name: skillpack-bootstrap
description: Set up a repository with the Skillpack before working – install the core memory skills (graphify, obsidian-memory, knowledge-vault, cloud-session-persistence, planning-with-files) plus the skill profile that fits the task (code, frontend, security, video, youtube), build the code graph and project vault, and add the session workflow to AGENTS.md/CLAUDE.md. Use at the START of any coding or project session when these skills/graphify-out/docs/vault are missing, when the user says "cài skillpack", "setup skill", "cài graphify obsidian", or before the first code change in a new repo.
---

# Skillpack bootstrap

Skillpack repo: `https://github.com/XclipseX/Skillpack-for-AI-Agent` (branch with latest work: see its README). Run this **once per repo**, then follow the session workflow it writes.

## 1. Check what is already there
```bash
ls .agents/skills .claude/skills 2>/dev/null | head; ls docs/vault/00-Index.md graphify-out/graph.json 2>/dev/null; grep -n "SKILLPACK-WORKFLOW" AGENTS.md CLAUDE.md 2>/dev/null
```
If core skills, vault, graph and the workflow block all exist → skip to the workflow (section 4).

## 2. Pick the profile from the task
| Task in the user's message | Profile |
|---|---|
| any code / app / script / repo work | `code` (always for coding) |
| website, UI, React, design, animation | `code,frontend` |
| audit, security review, CI with AI agents | `code,security` |
| video editing, motion, render | `video` |
| YouTube titles, shorts, transcripts, channel | `youtube` |
| unsure | `code` |
`core` is always installed (memory + persistence + skill finding).

## 3. Install (no questions needed – nothing here needs a login)
```bash
SP=/tmp/skillpack; [ -d "$SP" ] || git clone --depth 1 https://github.com/XclipseX/Skillpack-for-AI-Agent.git "$SP"
"$SP/install.sh" project . --profile code        # or code,frontend / video / youtube ...
```
`install.sh project` does: copy core + profile skills into `./.agents/skills` (and `./.claude/skills`), add the workflow block to `AGENTS.md` (+ `CLAUDE.md`), create `docs/vault/`, `pip install graphifyy` and run `graphify update .` (best effort – reports what failed).
If cloning is blocked: tell the user which host is blocked (github.com) and continue without the pack.
Then commit the added files (`.agents/`, `.claude/`, `AGENTS.md`, `CLAUDE.md`, `docs/vault/`; never `graphify-out/`) and tell the user in 1–2 lines what was installed.

## 4. Session workflow (also written into AGENTS.md)
- Start: read `docs/vault/00-Index.md` + latest `docs/vault/sessions/` note; refresh graph with `graphify update .` if stale; query it (`graphify query/explain/affected`) before reading many files.
- During: decisions → `docs/vault/decisions/`, bugs → `docs/vault/bugs/`.
- End: update session note + index, commit and push.
- Need another skill later? Use `find-skills` / `find-mcp-and-connectors`; skills needing a login or API key → ask the user first.
