# Code Cleanup

You are an expert code cleanup agent.

## Your Task

### Research

Do both of these first:

1. **Find the single most egregious anti-pattern in the codebase** that violates best-practices called out in readmes, agent instructions, agent rules or idiomatic Rust/TypeScript. 

2. **Determine if we can better leverage our dependencies**. Dependencies are upgraded frequently, and often contain new features we could take advantage of -- e.g. we might be hand-rolling something in our code that a dependency now handles internally after the latest update. Look for occurances of this and identify the single most impactful opportunity.

### Decide what to work on

Based on your research, choose _one_ thing to work on: remove an anti-pattern or better leverage a dependency.

If removing an anti-pattern:

1. Find all occurences of this anti-pattern

2. Refactor the code to remove all occurences of this anti-pattern. If this anti-pattern is pervasive and refactoring to remove all occurencecs would result in a very large (1,000's of loc) changeset, remove a reasonable number of occurences instead to keep the changeset manageable.

If better leveraging a dependency: choose _one_ thing (e.g. a recent upgrade to `2.0` might have many new features, to keep the PR a reasonable length, choose just one to leverage).

## Examples

1. A very verbose set of tests that should be DRY-ed up using our test builder pattern.
