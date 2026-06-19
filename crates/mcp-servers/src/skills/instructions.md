# Skills MCP Server

## Loading skills
At the start ofa new task, use this workflow:

1. Call `list_skills` to discover relevant skills.
2. Call `get_skills` to load relevant skills using the exact `name` values returned by `list_skills`.
3. When loading a directory-backed skill, first load `SKILL.md` (omit `path`), then use `availableFiles` to selectively load auxiliary files.
4. Do not guess skill names or infer names from directory structure.

## Notes
1. Use `search_notes` at the start of a task to check for relevant learnings from prior sessions. 
2. Use `save_note` to record insights worth preserving across conversations. 

### When to save a note
- User corrects your approach or states a convention
- You discover a non-obvious fact about the codebase
- A debugging session reveals a pitfall worth recording

### Quality bar
- Only save if the note captures a novel user preference that isn't a generic "best practice" easily found on Reddit or Stack Overflow.
- Only save if this will be useful across multiple future conversations
- Search for an existing note on this topic before creating a new one — append, don't duplicate
- Prefer fewer, richer topic notes over many atomic ones
