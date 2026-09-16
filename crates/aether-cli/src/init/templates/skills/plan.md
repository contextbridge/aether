---
name: plan
description: Research, write, and submit an implementation plan for review.
user-invocable: true
---

# Planner Instructions

You are in "plan mode" right now. When the user asks you to solve a problem your job is to come up with an implementation plan that we can hand off to a junior engineer to implement. Don't modify files, outside of writing and updating a markdown plan file. You exit "plan mode" once the user approves your plan.

## Workflow
Always follow this workflow when creating a plan.

### Step 1: Explore, Analyze and Research
First you must: 

- Understand the core objective(s) of the task
- Explore the codebase to ground yourself in existing architecture, code patterns, implementation details, and the files you'll need to modify.
- For complex tasks, use your web tools to explore best practices from popular open-source projects and well-respected engineering blogs that relate to the task.

### Step 2: Reflect

- Think deeply about the best way to solve the issue; favor simple solutions and using high-quality open source libraries over complexity and re-inventing our own solution.
- Ask the user clarifying questions if a) There are multiple ways to solve a problem and there isn't a clear "best" option, or b) There is ambiguity in the plan (junior engineers can't handle ambiguity and need to be told what to do).
- When you have enough information to draft your plan, proceed to the next step.

### Step 3: Generate Implementation Plan

Generate an implementation plan and save it with the `coding__write_file` tool in `docs/aether/plans`. Choose a short stable name such as `auth-refactor-plan.md`. Then present your plan to the user via the `review__review_artifact` tool using the plan path. It must include the following sections:

**Overview**
- Clear problem statement
- Success criteria and acceptance conditions

**Technical Approach**
- High-level architectural decisions
- Design patterns to employ
- Key technical considerations and trade-offs

**Implementation Steps**
- Numbered, sequential steps in logical order
- Each step should be atomic and completable independently where possible
- Include specific details: function names, class structures, API endpoints, pseudo-code

**Testing Plan**
- Unit tests required
- Integration tests needed
- Edge cases to verify

**Files to Modify/Create**
A markdown table that lists:

- The file's path
- The specific changes needed to the file
- Whether this file is being added, modified, or removed

**Additional Notes**
- Documentation updates needed
- Follow-up tasks that may be spawned

### User Feedback

Request user feedback. If you have a tool to do so, use it.

## Task

The task to plan is: <task>$ARGUMENTS</task>
