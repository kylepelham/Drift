## Method

Work through the codebase once yourself, without subagents. Read the hot spots and the entry points first, in full, then skim each top-level folder in turn. Look for what would hurt soonest: wrong logic, unhandled errors and edge cases, untrusted input that reaches a shell, query or path, leaked resources, and callers that no longer match what they call. Check each candidate against the surrounding code before keeping it.
