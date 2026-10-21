## Scope

Review `{target}` rather than the local changes:

- A pull request number or URL: `gh pr diff {target}`, with `gh pr view {target}` for its description. If `gh` is missing or signed out, say so and stop. When the pull request is not checked out, give lines as they are in its version of each file.
- A branch or tag: `git diff $(git merge-base HEAD {target}) {target}`. A single commit: `git show {target}`.
- A file or folder: the code as it is now, read in full.

Read the whole of a changed file where a hunk alone does not show what the code does.
