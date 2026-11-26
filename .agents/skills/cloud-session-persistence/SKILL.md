---
name: cloud-session-persistence
description: Keep work safe when the agent runs in an ephemeral cloud container (Claude Code on the web, Codex cloud, Gemini/Jules, Codespaces) that is wiped after inactivity. Use at the start and end of every cloud session, before long or risky tasks, after producing anything the user will want later (notes, vault, outputs, renders), and whenever the user asks "will this be lost?".
---

# Cloud Session Persistence

Rule #1: **anything not pushed to git (or uploaded to storage) is gone when the container is reclaimed.** Local files, installed packages, caches, background processes – all temporary.

## Classify what you produce
| Kind | Example | Where it must live |
|---|---|---|
| Source & knowledge | code, `docs/vault/` notes, ADRs, SKILL.md | **git** – commit + push |
| Regenerable cache | `.graph/`, `graphify-out/`, `node_modules/`, build output | nowhere – `.gitignore`, rebuild next session |
| Large binaries | videos, renders, datasets, models (> ~50 MB) | **external storage** (Drive/S3/R2 via `rclone`, or Git LFS), commit only a pointer/manifest |
| Secrets | API keys, tokens | environment secrets of the cloud platform – **never** git or vault |
| Tool installs | `pip install`, `apt install` | environment **setup script** so every new session has them |

## Session start (restore)
1. `git status && git log --oneline -5` – confirm branch and that previous work is there.
2. `git fetch origin <branch> && git merge FETCH_HEAD` if the remote branch moved.
3. Read `docs/vault/00-Index.md` + latest `sessions/` note (see obsidian-memory skill).
4. Rebuild caches that are needed (e.g. run the graphify skill).

## During work (checkpoint)
- Commit + push at every meaningful step (feature done, tests green, notes updated) – not only at the end. Small WIP commits are fine on a work branch.
- Before a long render/build/training job: push first, then run it.
- Write a short progress line to the session note as you go, so a cut-off session can resume.

## Session end (save)
1. Update the session note in the vault (goal, changes, open TODOs).
2. `git add -A && git status` – check nothing big or secret is staged (`git diff --cached --stat`).
3. Commit, `git push -u origin <branch>`; retry on network error.
4. Upload large outputs to external storage and record the link in the vault.
5. Tell the user what was saved and where.

## Setup for next sessions
- `.gitignore`: caches, `.env`, large media (`*.mp4 *.mov *.wav` unless using LFS).
- Put repeated installs (e.g. `pip install graphifyy`, `apt-get install -y ffmpeg`) in the platform's environment setup script, or a `scripts/setup.sh` committed to the repo.

## Never
- Leave the only copy of something in the container or in `/tmp`.
- Commit secrets or multi-GB files to plain git.
- Assume a background process survives the session.
