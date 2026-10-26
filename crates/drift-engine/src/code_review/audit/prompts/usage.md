Reviews the whole codebase as it stands, or one folder or file of it, rather than a change.

| Command | Description |
|---|---|
| `low [path]` | One pass without subagents, hot spots and entry points first |
| `medium [path]` | Finders over balanced areas and a verify pass, precision first (the default) |
| `high [path]` | More finders, keeping plausible findings too |
| `max [path]` | Every angle, then a sweep for what the list missed |

`--fix` applies the findings once they are reported.
