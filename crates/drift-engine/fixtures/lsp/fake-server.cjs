// A language server for tests: errors on lines holding ERROR, warnings on WARN. `mute` never initializes.
const mute = process.argv[2] === "mute"
let buffer = Buffer.alloc(0)
let configured = false

function send(message) {
  const body = Buffer.from(JSON.stringify(message))
  process.stdout.write(`Content-Length: ${body.length}\r\n\r\n`)
  process.stdout.write(body)
}

// Spelled as some servers spell it, drive letter lowercased and its colon escaped.
function respelled(uri) {
  return uri.replace(/^file:\/\/\/([A-Za-z]):/, (_, drive) => `file:///${drive.toLowerCase()}%3A`)
}

function publish(uri, text) {
  const diagnostics = []
  text.split(/\r?\n/).forEach((line, index) => {
    const range = { start: { line: index, character: 2 }, end: { line: index, character: 4 } }
    if (line.includes("ERROR")) diagnostics.push({ range, severity: 1, message: `bad:\n  ${line.trim()}` })
    if (line.includes("WARN")) diagnostics.push({ range, severity: 2, message: "only a warning" })
  })
  if (!configured) diagnostics.push({ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } }, severity: 1, message: "configuration request unanswered" })
  send({ jsonrpc: "2.0", method: "textDocument/publishDiagnostics", params: { uri: respelled(uri), diagnostics } })
}

function handle(message) {
  if (message.method === "initialize") {
    if (mute) return
    send({ jsonrpc: "2.0", id: message.id, result: { capabilities: { textDocumentSync: 1 } } })
    send({ jsonrpc: "2.0", id: "cfg-1", method: "workspace/configuration", params: { items: [{}, {}] } })
  } else if (message.id === "cfg-1") {
    configured = Array.isArray(message.result) && message.result.length === 2
  } else if (message.method === "textDocument/didOpen") {
    publish(message.params.textDocument.uri, message.params.textDocument.text)
  } else if (message.method === "textDocument/didChange") {
    publish(message.params.textDocument.uri, message.params.contentChanges[0].text)
  } else if (message.method === "shutdown") {
    send({ jsonrpc: "2.0", id: message.id, result: null })
  } else if (message.method === "exit") {
    process.exit(0)
  }
}

process.stdin.on("data", (chunk) => {
  buffer = Buffer.concat([buffer, chunk])
  for (;;) {
    const end = buffer.indexOf("\r\n\r\n")
    if (end < 0) return
    const length = Number(/Content-Length: (\d+)/i.exec(buffer.subarray(0, end).toString())?.[1])
    if (buffer.length < end + 4 + length) return
    handle(JSON.parse(buffer.subarray(end + 4, end + 4 + length).toString()))
    buffer = buffer.subarray(end + 4 + length)
  }
})
