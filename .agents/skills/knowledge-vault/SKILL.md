---
name: knowledge-vault
description: Personal knowledge management in an Obsidian vault – turn text, notes, links, meeting notes, transcripts or ideas the user sends into short, linked Markdown notes (wikilinks, tags, backlinks), find topic clusters, and report insights and knowledge gaps. Use when the user says "lưu vào Obsidian", "ghi chú lại", "đưa vào vault", "tạo note", "tổng hợp kiến thức", sends material to file away, or asks what their notes say about a topic. Not for code-project memory (use obsidian-memory for that).
---

# Knowledge Vault (Obsidian + graph)

Goal: every piece of material becomes **small, linked notes** in one vault, so Obsidian's graph (and Graphify, if available) shows the user's knowledge as connected clusters. Speak Vietnamese unless the user asks otherwise.

## 0. Vault location (decide once, then remember)
1. If a vault path was given before, or a folder with `00-Index.md` + `notes/` exists (default `knowledge/` at the repo root), use it.
2. Otherwise create `knowledge/` in the current repo and tell the user (one line). Ask only if the user clearly keeps the vault elsewhere.
3. Structure:
```
knowledge/
  00-Index.md      # map of content: topics → hub notes. Read first.
  notes/           # one concept / person / project / decision per note
  sources/         # one note per source (article, video, meeting, book)
  inbox/           # raw material not yet processed (optional)
```

## 1. Workflow for each piece of material
1. **Get clean content.** Link → extract readable text (use the `defuddle` skill/CLI if available, else a normal fetch); file → read it; YouTube → transcript (`youtube-transcript` skill if available).
2. **Understand it.** List: main topic, key concepts, people/orgs, projects, decisions, claims, open questions. Keep it to what the material actually says.
3. **Search the vault before writing** – for each concept: `grep -ril "<term>\|<synonym>" knowledge/` and check filenames/aliases.
   - Exists → **update** that note (add facts, sources, links). Don't create a duplicate.
   - Not found → create a new note.
   - Ask the user **only** when it's genuinely ambiguous (e.g. two notes could be the same concept).
4. **Write notes** (rules below), plus one `sources/` note for the material itself.
5. **Link both ways.** Every new note links to ≥1 existing note, and the existing note gets a link back (edit its "Related notes"). Add new hub topics to `00-Index.md`.
6. **Graph (optional, when the vault has ~30+ notes or the user asks):** if `graphify` is installed, run it on the vault (`/graphify knowledge/` inside the agent; it uses an LLM for Markdown – mention the cost) and use its clusters/hubs. Otherwise compute clusters yourself from the wikilinks (notes that link to each other most). Don't invent node/edge lists by hand; the links in the notes ARE the graph – Obsidian's Graph view renders them.
7. **Save.** In a git repo: commit (and push in cloud sessions – the container is temporary).

## 2. Note format
Filename: kebab-case, short, no date unless it is an event (`spaced-repetition.md`, `hop-ban-giao-q4-2026.md`).
```markdown
---
type: concept | person | project | decision | source | task
tags: [chu-de, ...]
aliases: [tên khác, viết tắt]
sources: ["[[ten-nguon]]"]
created: YYYY-MM-DD
updated: YYYY-MM-DD
---
# Tiêu đề

**Tóm tắt:** 1–3 câu.

## Nội dung chính
- Ý ngắn, mỗi ý một dòng. Ghi rõ đâu là dữ kiện từ nguồn, đâu là suy luận (*suy luận:* …).

## Related notes
- [[note-khac]] – quan hệ: thuộc về / hỗ trợ / mâu thuẫn với / gây ra / ví dụ của

## Sources
- [[ten-nguon]] (đoạn/phút nếu có)
```
Rules: one idea per note, < ~150 lines; split when longer. Relation words go next to the link (that is the "edge label"). Use Obsidian syntax correctly (`[[note]]`, `[[note|alias]]`, `![[embed]]`, `> [!note]` callouts – see `obsidian-markdown` skill if installed).

## 3. Honesty
- Never invent facts, quotes, dates or links. Missing info → write `[cần bổ sung]` and list it in the report.
- Keep the user's own opinions marked as theirs; keep source claims attributed to the source.
- No secrets or sensitive personal data in notes unless the user explicitly asks.

## 4. Report at the end of each turn (short)
1. **Notes:** created (new) / updated – as `[[links]]`.
2. **Map:** main clusters (topic → hub note → members), strongest connections, orphan notes.
3. **Insights & gaps (3–5):** contradictions between sources, concepts mentioned but without a note, questions the material raises, links worth adding.
4. **[cần bổ sung]** items, if any.
Then suggest one next step (e.g. "gửi tiếp nguồn X" or "tách note Y") – don't ask a multiple-choice question every turn.
