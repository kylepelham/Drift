## Method

Read the diff once yourself, top to bottom, without subagents, and skip hunks that only change tests. Look for what would hurt soonest: wrong logic, unhandled errors and edge cases, removed behaviour something still relies on, and calls that no longer match between the changed code and its callers. Check each candidate against the surrounding code before keeping it.
