# ACP: Drift in an editor

`drift-engined --acp` serves the [Agent Client Protocol](https://agentclientprotocol.com) (v1)
on stdin and stdout, so an editor such as Zed can run Drift as an external agent. It is the
same engine the app links, opened headless; ACP is a translation over its in-process calls,
never over HTTP. stdout carries only protocol messages, one JSON-RPC message per line; logs go
to stderr.

## Pointing Zed at it

Build the binary (`cargo build --release -p drift-engined`), then add a custom agent to Zed's
`settings.json` (Agent Settings, External Agents, Add Custom Agent):

```json
{
  "agent_servers": {
    "Drift": {
      "type": "custom",
      "command": "C:/path/to/drift-engined.exe",
      "args": ["--acp", "--model", "anthropic/claude-sonnet-4-5"],
      "env": {}
    }
  }
}
```

- `--model provider/model` is the model new sessions start on, split at the first slash
  (`openrouter/anthropic/claude-sonnet-4.5` is provider `openrouter`). Without it a turn uses the
  agent's or drift.json's `model`, and fails with a message saying so when neither has one.
- Sign-ins come from the operating system's credential store, which the app shares, or from a
  provider's key variable in `env` (such as `ANTHROPIC_API_KEY`). There is no ACP sign-in flow:
  `authMethods` is empty.
- `--data-dir DIR` picks where conversations are stored (`~/.local/share/drift` by default,
  apart from the app's). Do not point it at the app's data folder while the app runs: Drift keeps
  one writer per `drift.db`.
- Zed's `dev: open acp logs` shows every message both ways.

## What it supports

- `initialize`: protocol version 1, `loadSession`, image and embedded-context prompts.
- `session/new`: the `cwd` becomes a Drift workspace (added if Drift has none for it), and the
  session runs as the workspace's default agent, with its drift.json, AGENTS.md, rules and MCP
  servers.
- `session/load`: replays the conversation (user text, replies, reasoning, tool calls with their
  results, the todo list as a plan). The `cwd` must be the session's own workspace.
- `session/prompt`: text; images (`data` URLs); `resource_link` to a `file://` path, read as an
  `@` mention under Drift's own read rules; embedded resources, sent as the text the editor gave,
  which may be an unsaved buffer or a selection. Updates stream as `agent_message_chunk`,
  `agent_thought_chunk`, `tool_call` and `tool_call_update`, and `plan` from the todo list. The
  stop reason is `end_turn`, `cancelled`, `max_tokens` (the last reply hit its output limit),
  `refusal` (the provider's safety filter) or `max_turn_requests` (the turn paused itself, with
  the reason said as a message). A failed turn answers with a JSON-RPC error carrying its message.
- `session/cancel`: stops the turn as the app's Stop does; the prompt answers `cancelled`.
- `session/request_permission`: every ask Drift's rules leave to the user, the session's and its
  subagents', with "Allow once", "Always allow" (only when it would allow more than this call;
  kept as the workspace's grant, as in the app) and "Reject". A cancelled or failed answer
  rejects the call; the turn goes on.
- Tool calls carry a kind (`read`, `edit`, `execute`, `search`, `fetch`, `think`, `other`), the
  engine's title, the file locations they touch, live shell output while they run, and their
  result. An `edit` shows its replacement as a diff, and a `write` that creates a file shows it
  as a new file; an overwrite or a patch shows its result text.

## What it does not

- MCP servers the editor offers in `mcpServers` are not connected; a note goes to stderr. A
  workspace's servers come from drift.json, as in the app.
- No session modes, config options or model switching from the editor, no slash command list,
  no `session/list`, `resume`, `close` or `fork`.
- The editor's file system and terminal (`fs/*`, `terminal/*`) are never used: Drift reads,
  writes and runs commands itself, under its own permission rules and undo history.
- A question the model asks (the `question` tool) is declined, since ACP v1 has no such card;
  the model carries on without an answer.
- Audio prompts are refused.

## Why it is built this way

- Hand-written JSON-RPC instead of the `agent-client-protocol` crate: the subset Drift speaks
  is six methods and a handful of update shapes, about 120 lines of framing, while the crate is
  some twenty thousand lines that went through 84 releases and three major versions in its first
  fifteen months. `serde_json` and `tokio`, already in the tree, cover it.
- The translation lives in `drift-engined` (`src/acp/`), not the engine: the engine only gained
  `drift_engine::start`, the background work `listen` already ran, so a host that does not serve
  HTTP can run it too.
- A turn subscribes to the event hub before its prompt goes in and follows it until the session
  goes idle, asking the engine directly when the feed falls behind or goes quiet; updates and the
  response share one write queue, so the response always comes after the turn's last update.
- Each prompt is registered when its request is read, before it runs, so a `session/cancel`
  that follows it at once is never missed; one that lands before the turn starts still stops it.
- Tool call ids are the engine's part ids, unique for the life of the session, rather than the
  provider's call ids.
- When the client hangs up, its running turns stop as if cancelled, asks still waiting on it are
  refused, and the last messages are written before the process exits.
