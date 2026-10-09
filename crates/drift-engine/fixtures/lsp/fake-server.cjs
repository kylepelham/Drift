// A language server for tests: errors on lines holding ERROR, warnings on WARN. `mute` never initializes;
// `pull` publishes nothing and answers textDocument/diagnostic instead, registering it after initialize.
const mute = process.argv[2] === "mute";
const pull = process.argv[2] === "pull";
// `--count=<file>` adds a line to that file each time a server starts.
const count = process.argv.find((arg) => arg.startsWith("--count="))?.slice("--count=".length);
if (count) require("fs").appendFileSync(count, "started\n");
let buffer = Buffer.alloc(0);
let configured = false;
const texts = new Map();

function send(message) {
    const body = Buffer.from(JSON.stringify(message));
    process.stdout.write(`Content-Length: ${body.length}\r\n\r\n`);
    process.stdout.write(body);
}

// Spelled as some servers spell it, drive letter lowercased and its colon escaped.
function respelled(uri) {
    return uri.replace(/^file:\/\/\/([A-Za-z]):/, (_, drive) => `file:///${drive.toLowerCase()}%3A`);
}

function diagnose(text) {
    const diagnostics = [];
    text.split(/\r?\n/).forEach((line, index) => {
        const range = { start: { line: index, character: 2 }, end: { line: index, character: 4 } };
        if (line.includes("ERROR")) diagnostics.push({ range, severity: 1, message: `bad:\n  ${line.trim()}` });
        if (line.includes("WARN")) diagnostics.push({ range, severity: 2, message: "only a warning" });
    });
    if (!configured)
        diagnostics.push({
            range: { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } },
            severity: 1,
            message: "configuration request unanswered",
        });
    return diagnostics;
}

function changed(uri, text) {
    texts.set(uri, text);
    if (!pull)
        send({
            jsonrpc: "2.0",
            method: "textDocument/publishDiagnostics",
            params: { uri: respelled(uri), diagnostics: diagnose(text) },
        });
}

function handle(message) {
    if (message.method === "initialize") {
        if (mute) return;
        send({ jsonrpc: "2.0", id: message.id, result: { capabilities: { textDocumentSync: 1 } } });
        send({ jsonrpc: "2.0", id: "cfg-1", method: "workspace/configuration", params: { items: [{}, {}] } });
        if (pull)
            send({
                jsonrpc: "2.0",
                id: "reg-1",
                method: "client/registerCapability",
                params: { registrations: [{ id: "d", method: "textDocument/diagnostic" }] },
            });
    } else if (message.id === "cfg-1") {
        configured = Array.isArray(message.result) && message.result.length === 2;
    } else if (message.method === "textDocument/didOpen") {
        changed(message.params.textDocument.uri, message.params.textDocument.text);
    } else if (message.method === "textDocument/didChange") {
        changed(message.params.textDocument.uri, message.params.contentChanges[0].text);
    } else if (message.method === "textDocument/diagnostic") {
        const items = diagnose(texts.get(message.params.textDocument.uri) ?? "");
        send({ jsonrpc: "2.0", id: message.id, result: { kind: "full", items } });
    } else if (message.method === "shutdown") {
        send({ jsonrpc: "2.0", id: message.id, result: null });
    } else if (message.method === "exit") {
        process.exit(0);
    }
}

process.stdin.on("data", (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    for (;;) {
        const end = buffer.indexOf("\r\n\r\n");
        if (end < 0) return;
        const length = Number(/Content-Length: (\d+)/i.exec(buffer.subarray(0, end).toString())?.[1]);
        if (buffer.length < end + 4 + length) return;
        handle(JSON.parse(buffer.subarray(end + 4, end + 4 + length).toString()));
        buffer = buffer.subarray(end + 4 + length);
    }
});
