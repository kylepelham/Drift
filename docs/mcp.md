# MCP

The engine runs MCP servers itself (`crates/drift-engine/src/mcp`, on `rmcp`). Settings > MCP
manages them; every change goes through the engine's `/mcp` routes, so the desktop window and
a remote device see the same servers.

## Servers

- Each server is a row in `drift.db` (`mcp_config`): its name, its definition, whether it is
  on, and whether read-only agents may use its tools. A project's files can never add one.
- Transports: stdio (a command with arguments, environment variables and an optional working
  folder), streamable HTTP, and the deprecated HTTP with server-sent events. A remote server
  that answers in the 2026-07-28 stateless protocol is found by probing; the handshake is the
  fallback. Each row shows the transport and, once connected, the protocol version and mode.
- Environment variable and header values are secrets. They go into the engine and never come
  out: the API and its events carry the names only, and a save that sends a name without a
  value keeps the saved one.
- A remote server that asks for OAuth is signed in from its row; the token is kept in the
  operating system's credential store and renewed by the engine. A pre-registered OAuth app
  (client id, secret, scopes) can be set in the server's edit sheet.
- The row's switch turns a server on or off. The plug button connects or disconnects one
  that is on without changing its setting, and stays in place, greyed out, while the server
  is off. Saving a changed definition reconnects it.
- A stdio server runs once per workspace, in that folder (unless it sets its own working folder),
  with the folder as its root; it starts the first time a workspace needs it and keeps running
  while any window or device has that workspace open. It stops when the workspace is removed, or
  5 minutes after its last use once no window shows the workspace. Connecting from the manager starts it in
  the active workspace, so open a workspace first. Disconnecting stops it everywhere until you
  connect it again. A remote server has one connection for all workspaces.

## Tools, prompts and permissions

- A server's tools reach the model as `<server>_<tool>`, in characters every provider takes,
  and keep their names for the rest of the conversation once given.
- A call runs without asking unless a rule for permission kind `mcp` (pattern
  `<server>/<tool>`) says ask or deny; a tool a rule denies outright is never offered.
- Read-only agents (plan, explore) may use a server's read-only tools only when the server is
  trusted: the switch "Plan and Explore may use its read-only tools" in its edit sheet, on for
  a new server. A server's own read-only mark is its claim; the trust is the user's. A call is
  trusted only while its connection was opened from the definition saved now.
- A server's prompts appear as slash commands named `<server>:<prompt>`, their arguments
  filled word by word.
- Instructions a server sends when it connects go into the system prompt, but only for a turn
  that is offered that server's tools.

## Registry

The Registry tab lists GitHub's curated MCP servers, most popular first, with ranked search
and the official registry's matches appended. Installing one fills an install sheet for its
remote URL or its npx, uvx or docker command, asks for any value it needs first, and connects
it; a server that asks for sign-in has its sign-in page opened at once.

## Coming from Drift 1.3

Drift 1.3 ran MCP servers through opencode with an approval step. On first launch the importer
brings those servers over, and opencode's own, once each: a server stays on only if it was
enabled in opencode and approved in Drift's old approval step, matched by the exact
fingerprint that step recorded, so nothing that was never allowed starts by itself.
