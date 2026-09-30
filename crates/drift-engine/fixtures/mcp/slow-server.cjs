// An MCP server that takes its time initialising, so a config change can land while a connect is in flight.
const readline = require("node:readline")
const delay = Number(process.env.SLOW_MS ?? 1500)
const tool = process.env.TOOL_NAME ?? "old_tool"
const rl = readline.createInterface({ input: process.stdin })
const reply = (id, result) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id, result }) + "\n")
rl.on("line", (line) => {
  const message = JSON.parse(line)
  if (message.method === "initialize") return setTimeout(() => reply(message.id, { protocolVersion: "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: "slow", version: "0" } }), delay)
  if (message.method === "tools/list") return reply(message.id, { tools: [{ name: tool, description: "x", inputSchema: { type: "object" } }] })
  if (message.id !== undefined) reply(message.id, {})
})