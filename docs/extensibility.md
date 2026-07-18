# Extensibility

## Two plugin surfaces

1. Engine side: standard opencode plugins. Drift ships its own in `engine/opencode/`
   (injected via `OPENCODE_CONFIG_DIR`, which the engine treats as an extra config dir:
   it auto-discovers `plugin/*.ts` and installs `@opencode-ai/plugin` there). User
   plugins in `.opencode/` and global config work unchanged. Never patch
   `engine/upstream` for engine behavior; add a plugin here instead.
2. Drift side: UI/workflow hooks the engine cannot see. Modeled on claude-code's hook
   taxonomy (see `examples/claude-code/entrypoints/sdk/coreTypes.ts` HOOK_EVENTS).
   Not built yet; contract below.

## Planned Drift hook events

`thread.created`, `thread.selected`, `thread.archived`, `composer.submit` (can rewrite
or cancel), `message.rendered`, `part.render` (override renderer for a part/tool),
`permission.requested`, `session.idle`, `workspace.changed`, `theme.changed`.

Hooks are registered by Drift plugins: ESM modules listed in `drift.json`, loaded at
startup, given a typed `DriftApi` (engine actions, store snapshots, UI registration
points). No remote code; local files only.

## Spawned threads (shipped)

The claude-code Task tool spawns subagents that die with their result. Drift adds a
second primitive: `engine/opencode/plugin/spawn-thread.ts` registers a `spawn_thread`
tool the model calls with a title, task, its own context summary, and optional verbatim
excerpts (reasoning/CoT never crosses conversations; the model carries context as
plain text by design). The tool creates a sibling session in the same workspace, seeds
it with that carried context, and starts it on the parent's model. The new thread
appears in the sidebar like any other chat; the tool card links to it. Manual
counterpart: the fork button on a thread row duplicates a conversation with full
history.

## Workflows (phase 6)

Markdown files with frontmatter (name, description, inputs, steps) discovered from
`.drift/workflows` and exposed as slash commands. A workflow step is either a prompt
template or a tool invocation; state passes through the thread itself, mirroring
claude-code's skills-as-commands approach without a bespoke runtime.
