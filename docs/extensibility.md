# Extensibility (design, phases 5-6)

Status: design notes. Engine-side extensibility works today because it is opencode's;
Drift-side hooks and plugins are not built yet. This file is the contract to build to.

## Two plugin surfaces

1. Engine side: standard opencode plugins (`.opencode/plugin`, npm packages in
   opencode.json). They get `tool.execute.before/after`, `chat.*`, `permission.ask`,
   custom tools, auth, etc. Drift does nothing special; document and lean on it.
2. Drift side: UI/workflow hooks the engine cannot see. Modeled on claude-code's hook
   taxonomy (see `examples/claude-code/entrypoints/sdk/coreTypes.ts` HOOK_EVENTS).

## Planned Drift hook events

`thread.created`, `thread.selected`, `thread.archived`, `composer.submit` (can rewrite
or cancel), `message.rendered`, `part.render` (override renderer for a part/tool),
`permission.requested`, `session.idle`, `workspace.changed`, `theme.changed`.

Hooks are registered by Drift plugins: ESM modules listed in `drift.json`, loaded at
startup, given a typed `DriftApi` (engine actions, store snapshots, UI registration
points). No remote code; local files only.

## Spawned threads (phase 5)

The claude-code Task tool spawns subagents that die with their result. Drift adds a
second primitive: spawn a sibling session that lives in the sidebar like any other
thread, seeded with carried context (summary of the current thread plus user-picked
notes). Implementation: engine-side custom tool `spawn_thread` registered via an
opencode plugin (`tool` hook) that creates the session and prompts it; Drift recognises
the tool call and links the two threads in its store.

## Workflows (phase 6)

Markdown files with frontmatter (name, description, inputs, steps) discovered from
`.drift/workflows` and exposed as slash commands. A workflow step is either a prompt
template or a tool invocation; state passes through the thread itself, mirroring
claude-code's skills-as-commands approach without a bespoke runtime.
