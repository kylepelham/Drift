# Extensibility

## What can be extended

1. The engine: agents, commands and skills as Markdown files (`~/.config/drift/{agents,commands,skills}`
   for your own, `.drift/` in a project), `drift.json` settings (model, permission rules,
   providers, formatters, checks, language servers, instruction files, skill paths), MCP
   servers (see [mcp.md](mcp.md)), and per-family base prompts in Settings > Prompts. The engine
   runs no JavaScript and loads no plugins; its only plugin seam is an internal `Hook` trait,
   planned for M5 in `CHECKLIST.md`. Plugins written for opencode are named in the import
   summary and not run.
2. The interface: UI and workflow hooks the engine cannot see, modeled on claude-code's hook
   taxonomy (see `examples/claude-code/entrypoints/sdk/coreTypes.ts` HOOK_EVENTS). The Drift
   plugin foundation is built; the remaining planned events are listed below.
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

  api.registerToolContextActions("weather", (part) => ({
    id: "open-source",
    label: "Open weather source",
    run: () => api.files.open(part.state.input.filePath, { line: 1 }),
  }))
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

Context actions: `api.registerToolContextActions(toolName, fn)` adds right-click actions
to any tool card, including cards with plugin renderers. Use `"*"` to contribute an
action to every tool. Providers run when the menu opens and return one action, an array,
or `null`; each action has `id`, `label`, optional `detail`, `disabled`, and `separator`,
plus a sync or async `run` function. Registrations compose instead of replacing each
other and are removed automatically when the plugin unloads. `api.files.open(path,
{ line?, column? })` opens a local file; positioned opens prefer `DRIFT_EDITOR`,
`VISUAL`, or `EDITOR` when they contain a GUI executable path, then common installed
editors, and fall back to the system file association. Editor detection is cached and
launches the GUI executable directly, without command-shell probing or wrapper scripts.

Asks: `api.ask({ header, question, options: [{ label, description }], multiple?, custom? })`
(or an array of them) takes over the composer with the same card the engine's question
tool uses. The card renders option descriptions, single/multiple selection controls,
custom answers, question progress, and Back/Next/Submit navigation, then resolves with
the selected labels (`string[][]`, one array per question)
or `null` if dismissed. This is the intended plumbing for MCP-elicitation-style flows:
anything that needs a structured user answer shares one queue with engine questions and
permissions. The `question.requested` hook fires when the engine asks.

Hooks are registered by Drift plugins loaded from Drift's config directory. No remote
code; local files only.

## Slash completion and skill arguments

Tab completes a command or subcommand in the composer without executing it. Enter confirms the
highlighted choice or runs the completed command. Skill subcommand selections fill the draft first,
leaving room to add a target. Arrow keys navigate the scrollable list; Escape dismisses it.

The engine keeps a skill's `argument-hint` front matter as command usage. It finds subcommands in
explicit alternatives such as `[audit|polish]`, Markdown tables with `Command` and `Description`
columns, and inline invocations such as `/my-skill audit [target]` (`config::arguments`). Fenced
examples and other skills' commands are skipped. The workspace config (`GET /workspaces/{id}/config`)
returns them as each command's `usage` and `subcommands`, so no skill has to run for its argument
list to appear. Free-form hints such as `[target]` stay usage help, not invented choices. A command
whose template calls exactly one skill (`skill({ name: "..." })`) offers that skill's choices, even
under another name; its own template, agent, model and subtask settings still apply. A same-name
command that calls no skill inherits nothing. Argument choices use the same compact rows as `/fork`.
Argument names and descriptions stay on one line with ellipses. The disclosure arrow expands
the full details without selecting or running the command. At the end of the input, Right Arrow
expands the highlighted argument and Left Arrow collapses it. Editing the draft resets expansion.

Drift shows every matching command and subcommand, with descriptions and usage where documented.
For example, `/impeccable` followed by Tab opens its documented actions, including audit, critique,
polish, layout, and the other installed skill commands. Skills without argument metadata still
support completion and manually typed arguments.

## Branches and subagents (shipped)

A subagent works on the current goal; a branch pursues a different one. The engine keeps them
apart (see "Subagents and branches" in `docs/engine-rewrite.md`).

- Subagents come from the `task` tool. Their result returns to the parent's task card, which
  opens the stored transcript. They show under the parent in the sidebar while running, waiting on
  the user, or open, stop when the parent stops, and cannot delegate further.
- Spawned threads come only from the user. `/spawn <instruction>` creates a new top-level
  conversation at once with a copy of this one's finished messages and starts it on the
  instruction, on the source's model and level. There is no drafting request and no review: the
  new thread reads the copied conversation and works out what it needs. It records the source and
  the last message copied, runs independently, and has its own permissions. The model is not
  offered a tool to spawn.
- `read_thread` gives a conversation a one-shot snapshot of a branch taken from it: status,
  pending asks, todos and the latest reply. It refuses sessions that were not branched from the
  caller.

Forks copy a conversation's finished history into a new, independent conversation. A turn still
running is left out. The copy keeps compaction markers, so it continues from the same context;
`/fork active` and `/fork all` are one operation (see "Fork and move" in `docs/engine-rewrite.md`).

## Prompt and agent editing

Settings > Prompts edits the base prompt each model family starts with: GPT and Codex, Claude,
Gemini and other models, plus one for all models that a family's own replacement overrides.
They are the engine's (`GET`, `PUT` and `DELETE /prompts`); a replacement takes effect at each
conversation's next turn. The rules Drift always adds after the base prompt (tools and
`<system-reminder>`, the worktree, the shape of answers) are shown read-only and never replaced.

Settings > Agents edits each agent's prompt, model, steps, tools, permission rules and default
reasoning level. Drift keeps those edits in its store as `agent:<name>` overrides and hands them
to the engine, which applies them from the agent's next turn; a field the engine would not
apply is refused rather than stored. Reset removes the override and shows the agent as its
file or the built-in defines it. Saving refreshes the agent list for both desktop and
companion clients.
### Agent models

In Settings > Agents, select a subagent type such as `explore`, `general`, or a custom
agent, then choose its Model. The compact row aligns with the agent selector. The searchable list includes tool-capable models
from connected providers, including models hidden from the composer. LM Studio models
must meet its loaded-context requirement. Save the agent to apply to new tasks from idle
sessions. Tasks launched by an already active session keep that session's configuration
until its current work finishes.

Current model is the default for unpinned subagents. It uses the model of the session
invoking each task, not a snapshot of the model selected when the setting was saved.
An explicit choice stores the engine's `provider/model-id` under the existing SQLite
`agent:<name>` override. Selecting Current model stores an empty model string, which
masks any lower-precedence agent model and restores task model inheritance. Reset removes
the whole Drift agent override and restores the underlying agent configuration instead.
Prompt and behavior edits are preserved when changing the model. The picker and behavior
JSON edit the same value; unavailable saved models remain visible by ID until changed.

Built-in primary agents are `build`, `plan` (read-only) and `orchestrator`, which is offered no
tool that edits or runs commands and delegates every change to subagents; its replies end in a
status block. The engine reads it (`session/drive.rs`) and, while it says `working`, writes the
next prompt itself as a `nudge` part in the same turn, so the goal moves on with no client open.
A reply without a valid block gets a reminder of the protocol instead. The turn ends on `done`,
`blocked`, a failed reply, a Stop, or after 30 nudges since the user's own prompt; the user's next
message starts a fresh 30. The app only shows a notice for how the turn ended.

Built-in subagents are `general` (the default `task` type, full tools) and `explore` (read-only
search); a workspace `.drift/agents/<name>.md` with `mode: subagent` adds another. All appear in
Settings > Agents with their prompts and model pickers, never in the composer. A workspace agent
with `mode: all`, or no `mode` at all (as opencode reads one), is both: in the composer and offered
for delegation. `hidden: true` keeps one out of the composer; `disable: true` removes it. The engine runs a
`task` subagent on its agent's pinned model, falling back to the parent's.
Threads from `/spawn` start on the source conversation's model.

The action agents `title` and `compaction` have the same model picker and prompt editor. Title
defaults to the cheapest priced model from the conversation's provider, or the conversation's own
model when that is free; compaction defaults to the conversation's model. Their picker lists text models. The composer never
offers them as agents. Details: "Per-action models" in `docs/engine-rewrite.md`.

## Workflows (design open)

Not built yet; the shape is undecided. For reference, claude-code's WORKFLOW_SCRIPTS
(ant-only, runtime stubbed in our example copy) are markdown-config-defined runs that
execute as background tasks composed of per-step subagents (own transcripts, per-agent
skip/retry, run kill), surfaced both as slash commands and as a model-invocable tool.
A first Drift cut as markdown-steps-in-one-thread was built and removed; whatever lands
here should be designed against real orchestration needs, not guessed.