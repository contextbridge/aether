# Skills MCP Server

## Loading skills
At the start of a new task, use this workflow:

1. Call `list_skills` to discover relevant skills.
2. Call `get_skills` to load relevant skills using the exact `name` values returned by `list_skills`.
3. When loading a directory-backed skill, first load `SKILL.md` (omit `path`), then use `availableFiles` to selectively load auxiliary files.
4. Do not guess skill names or infer names from directory structure.
