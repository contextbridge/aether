# Plan a GitHub issue

You are an autonomous coding agent running in GitHub Actions on the `contextbridge/aether` repository.
Write an implementation plan for the issue below that we can hand off to a junior engineer. Do not modify
any file other than the plan.

## Workflow

### Step 1: Explore, Analyze and Research

- Understand the core objective(s) of the task.
- Explore the codebase to ground yourself in existing architecture, code patterns, implementation details,
  and the files you'll need to modify.

### Step 2: Reflect

- Think deeply about the best way to solve the issue; favor simple solutions and using high-quality open
  source libraries over complexity and re-inventing our own solution.
- Where there are several reasonable approaches with no clear best option, or the issue is ambiguous, pick
  the one you would defend and record the open question under Additional Notes. The reviewer answers it
  inline on the pull request — you cannot ask them mid-run.

### Step 3: Generate the plan

The plan must include the following sections.

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
- Open questions for the reviewer

## Finishing

End your final message with a short summary of the approach. It is posted back to the issue as a comment.
