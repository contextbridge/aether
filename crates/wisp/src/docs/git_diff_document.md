A parsed git diff for the current repository.

Produced by running `git diff` against the working tree and parsing the unified diff output. Contains the repository root path and a list of [`FileDiff`] entries, one per changed file.

# Structure

```text
GitDiffDocument
 └─ Vec<FileDiff>
     ├─ path, old_path, status, binary
     └─ Vec<Hunk>
         ├─ header, line ranges
         └─ Vec<PatchLine>
             └─ kind (Context | Added | Removed | HunkHeader | Meta) + text
```

# See also

- [`FileDiff`] — a single file's changes
- [`FileStatus`] — Modified, Added, Deleted, or Renamed
- [`GitDiffError`] — errors from git operations
