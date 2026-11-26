---
name: find-mcp-and-connectors
description: Find, compare and set up MCP servers, connectors, apps, extensions or plugins that give the agent access to an outside service (GitHub, Notion, Figma, databases, YouTube, Google Drive, Slack...). Use when the user asks "is there an MCP for X", "connect Claude/ChatGPT/Gemini to X", "find a connector/plugin/extension", or when a task needs a service the agent cannot reach yet. For finding SKILL.md skills use find-skills.
---

# Find MCP servers & connectors

## 1. Check what is already available
- Look at the tools/connectors already enabled in this session first – don't add a second way to reach the same service.
- Claude Code: `claude mcp list`; project config `.mcp.json`. Codex: `~/.codex/config.toml`. Gemini CLI: `.gemini/settings.json` / `gemini extensions list`. Cursor: `.cursor/mcp.json`.

## 2. Where to search (most trusted first)
| Kind | Where |
|---|---|
| Hosted connectors (one-click OAuth) | Claude: Settings → Connectors / Directory · ChatGPT: Apps & connectors · Gemini: Extensions |
| Official vendor MCP | the service's own docs/GitHub org (e.g. github/github-mcp-server, Figma, Stripe, Supabase, Cloudflare) |
| Official registry | https://registry.modelcontextprotocol.io (API: `/v0/servers?search=<term>`) and github.com/modelcontextprotocol/servers |
| Community catalogs | smithery.ai, glama.ai/mcp, mcp.so, awesome-mcp-servers lists – verify before use |
| Plugin marketplaces | `claude plugin marketplace`, `codex plugin`, `gemini extensions install`, `npx skills add` (skills.sh) |

Query the official registry (tested; public JSON, no key):
```bash
curl -s "https://registry.modelcontextprotocol.io/v0/servers?search=youtube&limit=20" |
  python3 -c "import json,sys;[print(x['server']['name'],'|',x['server'].get('description','')[:90],'|',x['server'].get('repository',{}).get('url')) for x in json.load(sys.stdin)['servers']]"
```
⚠️ The registry is **not curated**: expect keyword-stuffed names, paid/crypto "API hubs" and entries with no repository. Discard entries without a public repo or a known publisher.

## 3. Pick one – criteria
Prefer, in order: **official vendor** > hosted connector with OAuth > popular maintained open source (recent commits, many users, license) > anything else.
Prefer **remote/OAuth** over local servers that need a raw API key; prefer **read-only** scopes; one search/browser tool per profile is enough.
Every enabled MCP costs context (its tool schemas) – suggest only what the task needs and suggest disabling unused ones.

## 4. Before adding anything
- Run the **skill-vetting** checks on the repo/package (who publishes it, what it can access, install scripts).
- **Login / API key / OAuth needed → ask the user first**, say exactly which account and permissions. Never ask for secrets in chat; use the platform's connector flow or secret store.
- No login needed (local, read-only tool) → you may add it, then tell the user what you added and how to remove it.

## 5. Report
Give the user: name, publisher, link, what it can do, what it needs (account/key), how to install (one command or the settings path), and the alternative you rejected and why.
