#!/usr/bin/env bash
# Create an Obsidian-compatible memory vault. Usage: init_vault.sh [path]
set -euo pipefail
VAULT="${1:-docs/vault}"
TPL="$(cd "$(dirname "$0")/../.." && pwd)/.agents/skills/obsidian-memory/templates"
mkdir -p "$VAULT"/{architecture,decisions,sessions,bugs,snippets,templates}
cp -n "$TPL"/*.md "$VAULT/templates/" 2>/dev/null || true
if [ ! -f "$VAULT/00-Index.md" ]; then
cat > "$VAULT/00-Index.md" <<MD
---
type: index
updated: $(date +%F)
---
# Project Index (Map of Content)

> AI agents: read this note first, then the latest note in sessions/.

## Architecture

## Decisions

## Known bugs / gotchas

## Recent sessions
MD
fi
echo "Vault ready at $VAULT (open this folder in Obsidian as a vault)"
