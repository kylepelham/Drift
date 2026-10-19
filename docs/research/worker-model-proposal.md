# Models choosing a worker's model: proposal

Written 2026-10-19 for the "Design first" item in "After the opencode v2 review"
([engine-rewrite.md](../engine-rewrite.md)). No code until Kyle picks. Until then today's rule stays:
`task` has no model parameter, so the model can never send a worker to a provider of its choosing.

## The question

Should `task` take a `model` argument, and if so, whose choice may it carry: only a model Kyle named
in the conversation, or also one the model picks itself (a cheaper one, under a budget)? How does it
sit with the other places a worker's model comes from, with accounts, with a model listing, and in
the sidebar?

## How it works today

`Task::run` (`crates/drift-engine/src/tool/task.rs`) picks the worker's model in this order:

1. `ctx.command_model`: a user command that names a model (the `commandModel` in its bootstrap
   `task` call's metadata).
2. `config.agent_model(agent)`: Settings > Agents (stored as `prompt_override` `agent:<name>`,
   applied by `Engine::set_agent_overrides` over the agent file, so Settings wins; an empty model
   means "Current model" and masks the file's pin), else the agent file's `model:`.
3. The parent's model.

The worker runs at the parent's reasoning level (a model without it uses its default). A resumed
worker keeps its agent. The account is not chosen by `task` at all: the worker's turn picks the first
of that provider's accounts, in Kyle's order, with usage left and not refused the model
(`session/turn/accounts.rs`, `account_serves`). The sidebar shows a running or waiting worker as a
child row: an arrow (or a layers icon for background), a status dot and the title
`<description> (@<agent> subagent)` (`src/ui/workspaces.tsx`, `sidebarWorkers`).

## How v2 does it

`tool/plugin/subagent.ts` (#49194, 2026-09-16): `model` is optional, written
`providerID/modelID#variant`, described as "NEVER set this unless the user explicitly asks for a
particular model or variant ... look the model up with the models tool". The engine only checks the
model is available and the variant exists; nothing checks that the user asked. Precedence: the
argument, then the agent's model, then the parent's; continuing a child with a different model
switches it. A `models` tool (`tool/plugin/opencode.ts`) searches available models by name and
provider, newest first, with prices. The agent description also tells the model that a subagent
name it does not know is probably a model name.

## Options

**A. Keep today's rule.** Kyle routes work by pinning agents in Settings or with a command that
names a model. Nothing to build.

**B. A model Kyle named, checked by the engine.** `task` gains `model`. The engine accepts it only
when text Kyle wrote in this conversation names that model: by catalog id, display name or a unique
short form ("opus", "sol"), resolved against providers with a signed-in account. Tool output, file
contents and engine notes do not count, so a web page cannot name a model. Anything else fails with
"use a model the user named". This is v2's rule enforced instead of asked for.

**C. The model picks, under a budget.** Settings > Agents gains "Models a worker may be sent to"
(empty by default) and a per-conversation budget in dollars. The model may pass any listed model; a
launch that would pass the budget, counted with Drift's existing cost (subscription turns at API
rates), fails with the amount left. Option B's named models are always allowed as well.

| Point | A | B | C |
| --- | --- | --- | --- |
| Who decides | Kyle, ahead of time | Kyle, in the conversation | Kyle's list, then the model |
| Risk of an unexpected provider or bill | None | None beyond what he said | Bounded by list and budget |
| Fits "use Haiku for the searching" | Only by pinning explore | Yes | Yes, without saying it each time |
| Prompt cost | None | One optional field, about 300 bytes | The field, plus the list in the instruction baseline |

## Precedence with a `model` argument

1. A model Kyle named in the conversation (B), because it is the newest and most specific choice.
2. A command's model: it was Kyle's choice when he ran the command.
3. Settings > Agents pin, then the agent file's model.
4. Under C only: a model from the allowed list that the model picked, used only when nothing above
   applies, so a pin always beats the model's own choice.
5. The parent's model.

A resumed worker keeps its model unless the argument is a named model (B). The variant stays the
parent's unless the named form carries one (`#high`).

## Accounts

The argument picks a model, never an account: the worker uses that provider's accounts by the same
rule as any turn. A model whose provider has no account that can serve it fails before launch, with
the reason, rather than starting a worker that errors at its first request. The planned cache affinity (a
worker sends its parent's cache key) only helps on the same provider; on another, the worker starts
on a cold cache.

## Is a `models` tool needed

No. Under B the engine resolves what Kyle said; an ambiguous or unknown name fails with the few
closest catalog entries in the error, which is the listing exactly when it is needed. Under C the
allowed list sits in the instruction baseline with prices and changes as an instruction update, so
the model never has to search. A tool would add a definition to every request for a rare need.

## What the sidebar shows

The worker row stays as it is when the worker runs on its usual model (its pin, or the parent's).
When the argument changed it, the row adds the model's short name after the title and its tooltip
says why: "named by you" or "chosen within budget". The `task` row in the transcript shows the same,
and under C its tooltip also shows the budget left.

## Recommendation

Option B. It covers the case that comes up ("have a Haiku worker do the search", "ask Sol to review
this") with no new trust: the engine, not the description, holds the line v2 only asks the model to
respect. C can be added on top later if Kyle finds himself naming the same cheap model repeatedly;
the budget and list are the extra surface, and A stays the behaviour for anyone who never names a
model.

## What would be built (B)

- `model` on `task`'s schema: "Only a model the user named in this conversation, as they wrote it."
- A resolver in `session/tasks.rs`: the conversation's user-written text parts (the transcript is
  kept whole, so a summary does not hide them), matched against the catalog for signed-in providers;
  a unique match or a failure listing the candidates.
- The precedence above in `Task::run`, the account check before launch, and the model and reason in
  the task record and `ToolMetadata` for the UI.
- Sidebar and task row label as described.
- Tests: an unnamed model is refused; "use opus" resolves on a signed-in Anthropic account; a named
  model beats a Settings pin; a command's model beats a pin but not a named model; an ambiguous name
  lists candidates; a provider with no usable account fails before launch.

Rough size: 350 to 500 lines with tests, two to three days. Option C adds a Settings section, the
baseline list and the budget check, about 400 more lines.
