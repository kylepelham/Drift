## Method

### 1. Find

Cover every angle below. Split them across {finders}: `task` calls with `subagent_type` `explore`, made in one step so they run at once. Give each finder the scope, its angles, the conventions above, and these rules: name the file and line, state the failure in one sentence with the inputs or state that cause it, return at most 6 candidates, and edit nothing.

{angles}

If the `task` tool is not available, work through each angle yourself in turn.
