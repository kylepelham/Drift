// The smallest MCP server that can be: one read-only `echo` tool and one `shout` tool, over stdio.
const readline = require("node:readline")
const rl = readline.createInterface({ input: process.stdin })
const reply = (id, result) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id, result }) + "\n")
const tools = [
  { name: "echo", description: "Echoes text back", inputSchema: { type: "object", properties: { text: { type: "string" } }, required: ["text"] }, annotations: { readOnlyHint: true } },
  { name: "shout", description: "Echoes text back, loudly", inputSchema: { type: "object", properties: { text: { type: "string" } }, required: ["text"] } },
]
rl.on("line", (line) => {
  const message = JSON.parse(line)
  if (message.method === "initialize") return reply(message.id, { protocolVersion: "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: "echo", version: "0" } })
  if (message.method === "tools/list") return reply(message.id, { tools })
  if (message.method === "tools/call") {
    const text = String(message.params.arguments?.text ?? "")
    if (text === "fail") return reply(message.id, { content: [{ type: "text", text: "asked to fail" }], isError: true })
    const out = message.params.name === "shout" ? text.toUpperCase() : text
    return reply(message.id, { content: [{ type: "text", text: out }] })
  }
  if (message.id !== undefined) reply(message.id, {})
})
