// The smallest MCP server that can be: one read-only `echo` tool and one `shout` tool, over stdio.
const fs = require("node:fs")
const readline = require("node:readline")
const rl = readline.createInterface({ input: process.stdin })
const reply = (id, result) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id, result }) + "\n")
// REDEFINE_AFTER_CRASH: once CRASH_MARKER exists, the restarted server no longer calls echo read-only.
const redefined = !!process.env.REDEFINE_AFTER_CRASH && !!process.env.CRASH_MARKER && fs.existsSync(process.env.CRASH_MARKER)
const tools = [
  { name: "echo", description: "Echoes text back", inputSchema: { type: "object", properties: { text: { type: "string" } }, required: ["text"] }, annotations: { readOnlyHint: !redefined } },
  { name: "shout", description: "Echoes text back, loudly", inputSchema: { type: "object", properties: { text: { type: "string" } }, required: ["text"] } },
]
// Texts: "fail" is a tool error, "crash" exits, "crash-once" exits only while CRASH_MARKER is absent, "hang" never answers,
// "picture" answers with text and a PNG.
const call = (message) => {
  const text = String(message.params.arguments?.text ?? "")
  if (process.env.CALL_LOG) fs.appendFileSync(process.env.CALL_LOG, `${message.params.name} ${text}\n`)
  if (text === "fail") return reply(message.id, { content: [{ type: "text", text: "asked to fail" }], isError: true })
  if (text === "picture") return reply(message.id, { content: [{ type: "text", text: "a screenshot" }, { type: "image", mimeType: "image/png", data: "iVBORw0KGgo=" }] })
  if (text === "crash") process.exit(1)
  if (text === "crash-once" && !fs.existsSync(process.env.CRASH_MARKER)) {
    fs.writeFileSync(process.env.CRASH_MARKER, "")
    process.exit(1)
  }
  if (text === "hang") return
  const out = message.params.name === "shout" ? text.toUpperCase() : text
  reply(message.id, { content: [{ type: "text", text: out }] })
}
// RICH: also serve one resource of each kind and one prompt with two arguments.
const rich = !!process.env.RICH
const capabilities = rich ? { tools: {}, resources: {}, prompts: {} } : { tools: {} }
const resources = [{ uri: "note://readme", name: "readme", mimeType: "text/plain", description: "The notes" }, { uri: "note://shot", name: "shot", mimeType: "image/png" }]
const contents = { "note://readme": [{ uri: "note://readme", mimeType: "text/plain", text: "remember the milk" }], "note://shot": [{ uri: "note://shot", mimeType: "image/png", blob: "iVBORw0KGgo=" }] }
const prompt = { name: "review", description: "Review a file", arguments: [{ name: "file", required: true }, { name: "focus" }] }
const filled = (args) => ({ messages: [{ role: "user", content: { type: "text", text: `Review ${args.file} for ${args.focus ?? "anything"}` } }] })
rl.on("line", (line) => {
  const message = JSON.parse(line)
  if (message.method === "initialize") return reply(message.id, { protocolVersion: "2025-06-18", capabilities, serverInfo: { name: "echo", version: "0" }, instructions: "Echo repeats what it is given." })
  if (message.method === "tools/list") return reply(message.id, { tools })
  if (message.method === "tools/call") return call(message)
  if (message.method === "resources/list") return reply(message.id, { resources })
  if (message.method === "resources/read") return reply(message.id, { contents: contents[message.params.uri] ?? [] })
  if (message.method === "prompts/list") return reply(message.id, { prompts: [prompt] })
  if (message.method === "prompts/get") return reply(message.id, filled(message.params.arguments ?? {}))
  if (message.id !== undefined) reply(message.id, {})
})
