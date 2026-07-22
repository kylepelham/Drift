# Extensibility

## Two plugin surfaces

1. Engine side: standard opencode plugins. Drift ships its own in `engine/opencode/`
   (injected via `OPENCODE_CONFIG_DIR`, which the engine treats as an extra config dir:
   it auto-discovers `plugin/*.ts`, reads its `opencode.json`, and installs
   `@opencode-ai/plugin` there). That `opencode.json` also pins npm plugins Drift ships
   by default, currently `@ex-machina/opencode-anthropic-auth` so Claude Pro/Max plan
   sign-ins work out of the box (installed on demand into the opencode package cache).
   User plugins in `.opencode/` and global config work unchanged. Never patch
   `engine/upstream` for engine behavior; add a plugin here instead.
2. Drift side: UI/workflow hooks the engine cannot see. Modeled on claude-code's hook
    taxonomy (see `examples/claude-code/entrypoints/sdk/coreTypes.ts` HOOK_EVENTS).
    The Drift plugin foundation is built; the remaining planned events are listed
    below.

## Drift plugins

Drift's platform config directory can list local JavaScript modules in `drift.json`:

```json
{
  "plugins": ["plugins/example.mjs"]
}
```

Paths are relative to that config directory, must stay inside it, and must end in `.js`
or `.mjs`. Entry modules are self-contained ESM files with a default function. A cloned
workspace can never make Drift execute plugin code merely by being opened.

```js
export default function (api) {
  api.on("composer.submit", ({ text }) => {
    if (text === "!!ping") return "Say exactly: pong"
  })

  api.registerToolRenderer("weather", (part) => {
    const row = document.createElement("div")
    row.textContent = part.state.status === "completed" ? part.state.output : "Loading weather..."
    return row
  })
}
```

`api.version` is `1`. `api.context()` returns the active workspace, selected thread,
and engine connection state. `api.threads.create()` creates and selects a thread;
`api.threads.select(id)` changes the selected thread. Renderers may return a DOM node,
plain text, or `null`; strings are never treated as HTML.

Hook events: `composer.submit`, `thread.created`, `thread.selected`, `thread.archived`,
`workspace.changed`, `theme.changed`, `message.rendered` (a message entered the
transcript DOM), `permission.requested` (any session, subagents included), and
`session.idle` (a busy session finished). Composer hooks run in registration order and
may return replacement text or `false` to cancel submission. Other hooks are
notifications. Plugin failures are isolated and logged to the browser console.

Renderers: `api.registerToolRenderer(toolName, fn)` overrides the card body for a tool;
`api.registerPartRenderer(partType, fn)` renders non-tool part types (for example
`reasoning` or `file`), including types Drift normally hides. Tool parts always go
through tool renderers, never part renderers.

Asks: `api.ask({ header, question, options: [{ label, description }], multiple?, custom? })`
(or an array of them) takes over the composer with the same card the engine's question
tool uses and resolves with the selected labels (`string[][]`, one array per question)
or `null` if dismissed. This is the intended plumbing for MCP-elicitation-style flows:
anything that needs a structured user answer shares one queue with engine questions and
permissions. The `question.requested` hook fires when the engine asks.

Hooks are registered by Drift plugins loaded from Drift's config directory. No remote
code; local files only.

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

## Workflows (design open)

Not built yet; the shape is undecided. For reference, claude-code's WORKFLOW_SCRIPTS
(ant-only, runtime stubbed in our example copy) are markdown-config-defined runs that
execute as background tasks composed of per-step subagents (own transcripts, per-agent
skip/retry, run kill), surfaced both as slash commands and as a model-invocable tool.
A first Drift cut as markdown-steps-in-one-thread was built and removed; whatever lands
here should be designed against real orchestration needs, not guessed.
