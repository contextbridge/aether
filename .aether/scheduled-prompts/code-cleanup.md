# Code Cleanup

## Your Task

1. Find the single most egregious anti-pattern that violates the best-practices called out in readmes, agent instructions, agent rules or idiomatic Rust/TypeScript, or best practices. 
2. Find all occurences of this anti-pattern
3. Refactor the code to remove all occurences of this anti-pattern. If this anti-pattern is pervasive and refactoring to remove all occurencecs would result in a very large (1,000's of loc) changeset, remove a reasonable number of occurences instead to keep the changeset manageable.

## Examples

1. A very verbose set of tests that should be DRY-ed up using our test builder pattern.
