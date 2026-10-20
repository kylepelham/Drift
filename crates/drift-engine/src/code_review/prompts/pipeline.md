## Method

### 1. Find

Cover every angle below. Split them across {finders}: `task` calls with `subagent_type` `explore`, made in one step so they run at once. Give each finder the scope, its angles, the conventions above, and these rules: name the file and line, state the failure in one sentence with the inputs or state that cause it, return at most 6 candidates, and edit nothing.

{angles}

If the `task` tool is not available, work through each angle yourself in turn.

### 2. Verify

Merge duplicate candidates, then check them in batches: one `explore` subagent for every five or so candidates, asked to read the code and give each one a verdict with a line of evidence. Confirmed: the failure happens as described. Plausible: it can happen, but depends on something the code does not show. Refuted: it cannot happen, or the code already handles it. Drop refuted candidates. {keep}
