Loads content for skills discovered via `list_skills`.

Use the exact `name` values returned by `list_skills`. Only skills marked `agent-invocable: true` can be loaded.

If you omit `path`, this loads the skill root content (`SKILL.md` for directory-backed skills, or the markdown file for flat skills). If you provide `path`, it loads that file relative to a directory-backed skill root.

When loading a directory-backed `SKILL.md`, the server returns `availableFiles` so you can progressively load only the auxiliary files you need.

## Usage

```json
{
  "requests": [
    { "name": "rust" },
    { "name": "rust", "path": "traits.md" }
  ]
}
```

## Parameters

- `requests` — **required**, array of skill requests
  - `name` — **required**, exact skill name returned by `list_skills`
  - `path` — optional, file path relative to the skill root (for directory-backed skills)
