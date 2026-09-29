// Engine baselines: cold start, prompt overhead around a stub provider, and per-turn prompt size.
// `bun run bench:engine [opencode|native] [runs]`. Results feed docs/engine-rewrite.md.
import { existsSync, mkdtempSync, rmSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import { engineBinary, engineEnv, prepareRuntime } from "./engine-runtime"

const root = path.resolve(import.meta.dirname, "..")
const target = process.argv[2] ?? "opencode"
const runs = Number(process.argv[3] ?? 5)
const workspace = mkdtempSync(path.join(os.tmpdir(), "drift-bench-workspace-"))

type Result = { coldStartMs: number; promptToProviderMs?: number; providerToEventMs?: number; systemChars?: number; toolsChars?: number }

const median = (values: number[]) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)]
const summarise = (results: Result[]) => {
  const pick = (key: keyof Result) => {
    const values = results.map((r) => r[key]).filter((v): v is number => typeof v === "number")
    return values.length ? median(values) : undefined
  }
  return {
    target,
    runs,
    coldStartMs: pick("coldStartMs"),
    promptToProviderMs: pick("promptToProviderMs"),
    providerToEventMs: pick("providerToEventMs"),
    systemChars: pick("systemChars"),
    toolsChars: pick("toolsChars"),
    approxTokensPerTurn: Math.round(((pick("systemChars") ?? 0) + (pick("toolsChars") ?? 0)) / 4),
  }
}

const userPrompt = "Reply with ok."

type ChatRequest = { at: number; messages: { role: string; content: string }[]; tools: unknown[] }

/** An OpenAI-compatible endpoint that answers instantly so only engine time is measured. */
function stubProvider() {
  const requests: ChatRequest[] = []
  const server = Bun.serve({
    port: 0,
    fetch: async (request) => {
      const at = performance.now()
      const body = (await request.json()) as { messages?: ChatRequest["messages"]; tools?: unknown[] }
      requests.push({ at, messages: body.messages ?? [], tools: body.tools ?? [] })
      if (process.env.BENCH_DEBUG) console.error("stub", new URL(request.url).pathname, JSON.stringify(body).slice(0, 300))
      const chunk = (delta: object, finish: string | null = null) =>
        `data: ${JSON.stringify({ id: "b", object: "chat.completion.chunk", created: 0, model: "stub", choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`
      const text = chunk({ role: "assistant", content: "" }) + chunk({ content: "ok" }) + chunk({}, "stop") + "data: [DONE]\n\n"
      return new Response(text, { headers: { "content-type": "text/event-stream" } })
    },
  })
  // The engine also asks the model for a thread title; only the turn that carries tools is the real one.
  const turn = () => {
    const found = requests.find((r) => r.tools.length > 0 && r.messages.some((m) => m.role === "user" && String(m.content).includes(userPrompt)))
    if (!found) throw new Error("stub never received the chat turn")
    return found
  }
  return {
    url: `http://127.0.0.1:${server.port}/v1`,
    requestAt: () => turn().at,
    systemChars: () => turn().messages.filter((m) => m.role === "system").reduce((n, m) => n + String(m.content).length, 0),
    toolsChars: () => JSON.stringify(turn().tools).length,
    stop: () => server.stop(true),
  }
}

async function readUntil<T>(stdout: ReadableStream<Uint8Array>, matches: (text: string) => T | undefined) {
  let buffered = ""
  for await (const chunk of stdout) {
    buffered += new TextDecoder().decode(chunk)
    const found = matches(buffered)
    if (found !== undefined) return found
  }
  throw new Error("process exited before reporting")
}

async function benchOpencode(): Promise<Result> {
  if (!existsSync(engineBinary)) throw new Error("drift-engine.exe missing; run bun run build:engine")
  const stub = stubProvider()
  const config = await prepareRuntime({
    provider: {
      bench: {
        npm: "@ai-sdk/openai-compatible",
        name: "Bench",
        options: { baseURL: stub.url, apiKey: "bench" },
        models: { stub: { name: "stub", tools: true, limit: { context: 128000, output: 8192 } } },
      },
    },
  })
  const password = "bench"
  const started = performance.now()
  const proc = Bun.spawn([engineBinary, "serve", "--hostname", "127.0.0.1", "--port", "0"], {
    cwd: workspace,
    stdout: "pipe",
    stderr: "ignore",
    env: engineEnv(config, password),
  })
  const headers = {
    authorization: `Basic ${btoa(`opencode:${password}`)}`,
    "x-opencode-directory": encodeURIComponent(workspace),
    "content-type": "application/json",
  }
  try {
    const url = await readUntil(proc.stdout, (text) => text.match(/listening on (http\S+)/)?.[1])
    const events = await fetch(`${url}/global/event`, { headers })
    const reader = events.body!.getReader()
    const decoder = new TextDecoder()
    let buffered = ""
    const waitFor = async (predicate: (event: { type: string; properties?: Record<string, unknown> }) => boolean) => {
      for (;;) {
        const { value, done } = await reader.read()
        if (done) throw new Error("event stream closed")
        buffered += decoder.decode(value, { stream: true })
        const lines = buffered.split("\n")
        buffered = lines.pop() ?? ""
        for (const line of lines) {
          if (!line.startsWith("data: ")) continue
          const payload = JSON.parse(line.slice(6)).payload
          if (process.env.BENCH_DEBUG && payload?.type !== "server.heartbeat") console.error("event", line.slice(6, 400))
          if (payload && predicate(payload)) return performance.now()
        }
      }
    }
    await waitFor((event) => event.type === "server.connected")
    const coldStartMs = performance.now() - started

    const created = await fetch(`${url}/session`, { method: "POST", headers, body: "{}" })
    if (!created.ok) throw new Error(`session create ${created.status}: ${await created.text()}`)
    const session = (await created.json()) as { id: string }
    const promptAt = performance.now()
    const prompted = await fetch(`${url}/session/${session.id}/prompt_async`, {
      method: "POST",
      headers,
      body: JSON.stringify({ parts: [{ type: "text", text: userPrompt }], model: { providerID: "bench", modelID: "stub" } }),
    })
    if (!prompted.ok) throw new Error(`prompt ${prompted.status}: ${await prompted.text()}`)
    const assistant = new Set<string>()
    const textAt = await waitFor((event) => {
      const info = event.properties?.info as { id: string; role: string } | undefined
      if (event.type === "message.updated" && info?.role === "assistant") assistant.add(info.id)
      const part = event.properties?.part as { type?: string; text?: string; messageID?: string } | undefined
      return event.type === "message.part.updated" && part?.type === "text" && Boolean(part.text) && assistant.has(part.messageID ?? "")
    })
    return {
      coldStartMs,
      promptToProviderMs: stub.requestAt() - promptAt,
      providerToEventMs: textAt - stub.requestAt(),
      systemChars: stub.systemChars(),
      toolsChars: stub.toolsChars(),
    }
  } finally {
    proc.kill()
    await proc.exited
    stub.stop()
    rmSync(config, { recursive: true, force: true })
  }
}

async function benchNative(): Promise<Result> {
  Bun.spawnSync(["cargo", "build", "-q", "--release", "-p", "drift-engined"], { cwd: root, stdout: "inherit", stderr: "inherit" })
  const data = mkdtempSync(path.join(os.tmpdir(), "drift-bench-native-"))
  const started = performance.now()
  const proc = Bun.spawn([path.join(root, "target", "release", "drift-engined.exe"), "--data-dir", data], { stdout: "pipe", stderr: "ignore" })
  try {
    const { url, token } = await readUntil(proc.stdout, (text) => {
      const url = text.match(/url (\S+)/)?.[1]
      const token = text.match(/token (\S+)/)?.[1]
      return url && token ? { url, token } : undefined
    })
    await new Promise<void>((resolve, reject) => {
      const socket = new WebSocket(`${url.replace(/^http/, "ws")}/events?token=${token}`)
      socket.onmessage = () => {
        socket.close()
        resolve()
      }
      socket.onerror = () => reject(new Error("event socket failed"))
    })
    return { coldStartMs: performance.now() - started }
  } finally {
    proc.kill()
    await proc.exited
    rmSync(data, { recursive: true, force: true })
  }
}

const bench = target === "native" ? benchNative : benchOpencode
const results: Result[] = []
for (let run = 0; run < runs; run += 1) results.push(await bench())
rmSync(workspace, { recursive: true, force: true })
console.log(JSON.stringify(summarise(results), null, 2))
