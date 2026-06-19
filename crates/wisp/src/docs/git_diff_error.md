Errors that can occur when producing a [`GitDiffDocument`].

# Variants

- **`NotARepository`** — the working directory is not inside a git repository.
- **`CommandFailed`** — `git diff` exited with an error; carries the stderr output.
- **`ParseError`** — the diff output could not be parsed into structured hunks.
