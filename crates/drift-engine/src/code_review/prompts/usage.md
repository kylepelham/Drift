Reviews the branch's work and its uncommitted changes, or a target: a pull request number, a branch or commit, or a path.

| Command | Description |
|---|---|
| `low [target]` | One quick pass without subagents |
| `medium [target]` | Finders and a verify pass, precision first (the default) |
| `high [target]` | More finders, keeping plausible findings too |
| `max [target]` | Every angle, then a sweep for what the list missed |

`--fix` applies the findings once they are reported; `--comment` posts them on the branch's pull request.
