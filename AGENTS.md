<!-- SKILLPACK-WORKFLOW:BEGIN (managed by Skillpack install.sh – edit outside this block) -->
## AI session workflow (Skillpack)
Skills live in `.agents/skills/` (also `.claude/skills/`). Use them in every session:
- **Start – obsidian-memory:** read `docs/vault/00-Index.md` and the latest note in `docs/vault/sessions/`; grep the vault for the task's keywords.
- **Start – graphify:** if `graphify-out/graph.json` is missing or older than HEAD, run `graphify update .` (`pip install graphifyy`). Use `graphify query "…" / explain / affected` instead of reading many files.
- **During:** non-obvious decisions → `docs/vault/decisions/ADR-xxxx-*.md`; bugs + root cause → `docs/vault/bugs/`. Big multi-step tasks → **planning-with-files**.
- **End – obsidian-memory + cloud-session-persistence:** update `docs/vault/sessions/<date>.md` (goal, changes, TODOs) and `00-Index.md`, then commit and push. Nothing durable stays only in the container.
- User sends notes/links/ideas to keep → **knowledge-vault**. Missing a capability → **find-skills** / **find-mcp-and-connectors** (check with **skill-vetting**). Skills that need a login/API key: ask the user first; everything else: install and report.
- Communicate with the user in their language (Vietnamese for this owner).
<!-- SKILLPACK-WORKFLOW:END -->
