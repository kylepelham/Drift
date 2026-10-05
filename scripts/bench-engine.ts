// Engine baselines: cold start, prompt overhead around a stub provider, and per-turn prompt size.
// `bun run bench:engine [runs]`. Results feed the Baselines table in docs/engine-rewrite.md, beside
// the opencode 1.18.33 numbers recorded at M0.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"

const root = path.resolve(import.meta.dirname, "..")
const runs = Number(process.argv[2] ?? 5)

type Result = { coldStartMs: number; promptToProviderMs: number; providerToEventMs: number; systemChars: number; toolsChars: number }

const median = (values: number[]) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)]

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
      const chunk = (delta: object, finish: string | null = null) =>
        `data: ${JSON.stringify({ id: "b", object: "chat.completion.chunk", created: 0, model: "stub", choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`
      const text = chunk({ role: "assistant", content: "" }) + chunk({ content: "ok" }) + chunk({}, "stop") + "data: [DONE]\n\n"
      return new Response(text, { headers: { "content-type": "text/event-stream" } })
    },
  })
  // The engine also asks the model for a thread title; only the request that carries tools is the turn.
  const turn = () => {
    const found = requests.find((r) => r.tools.length > 0 && r.messages.some((m) => m.role === "user" && JSON.stringify(m.content).includes(userPrompt)))
    if (!found) throw new Error("stub never received the chat turn")
    return found
  }
  return {
    url: `http://127.0.0.1:${server.port}/v1`,
    requestAt: () => turn().at,
    systemChars: () => turn().messages.filter((m) => m.role === "system").reduce((n, m) => n + JSON.stringify(m.content).length - 2, 0),
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

/** A home folder whose drift.json adds the stub as a provider, so the engine sees it as a user's own. */
function benchHome(stubUrl: string) {
  const home = mkdtempSync(path.join(os.tmpdir(), "drift-bench-home-"))
  mkdirSync(path.join(home, ".config", "drift"), { recursive: true })
  const providers = { bench: { name: "Bench", baseUrl: stubUrl, apiKeyEnv: "DRIFT_BENCH_KEY", models: { stub: { context: 128000, output: 8192 } } } }
  writeFileSync(path.join(home, ".config", "drift", "drift.json"), JSON.stringify({ providers }))
  return home
}

async function bench(): Promise<Result> {
  const stub = stubProvider()
  const home = benchHome(stub.url)
  const data = mkdtempSync(path.join(os.tmpdir(), "drift-bench-data-"))
  const workspace = mkdtempSync(path.join(os.tmpdir(), "drift-bench-workspace-"))
  const started = performance.now()
  const proc = Bun.spawn([path.join(root, "target", "release", "drift-engined.exe"), "--data-dir", data], {
    stdout: "pipe",
    stderr: "ignore",
    env: { ...process.env, USERPROFILE: home, HOME: home, DRIFT_BENCH_KEY: "bench" },
  })
  try {
    const { url, token } = await readUntil(proc.stdout, (text) => {
      const url = text.match(/url (\S+)/)?.[1]
      const token = text.match(/token (\S+)/)?.[1]
      return url && token ? { url, token } : undefined
    })
    const frames: { at: number; text: string }[] = []
    const socket = new WebSocket(`${url.replace(/^http/, "ws")}/events?token=${token}`)
    await new Promise<void>((resolve, reject) => {
      socket.onmessage = (message) => {
        frames.push({ at: performance.now(), text: String(message.data) })
        resolve()
      }
      socket.onerror = () => reject(new Error("event socket failed"))
    })
    const coldStartMs = performance.now() - started
    const call = async (method: string, route: string, body: unknown) => {
      const response = await fetch(`${url}${route}`, { method, headers: { authorization: `Bearer ${token}`, "content-type": "application/json" }, body: JSON.stringify(body) })
      if (!response.ok) throw new Error(`${route} ${response.status}: ${await response.text()}`)
      const text = await response.text()
      return text ? JSON.parse(text) : undefined
    }
    const created = await call("POST", "/workspaces", { path: workspace, name: "bench" })
    const session = await call("POST", "/sessions", { workspaceId: created.id, title: "Bench", model: { provider: "bench", model: "stub" } })
    const promptAt = performance.now()
    await call("POST", `/sessions/${session.id}/turns`, { parts: [{ type: "text", text: userPrompt }] })
    const textAt = await new Promise<number>((resolve, reject) => {
      const deadline = setTimeout(() => reject(new Error("no text event within 30 s")), 30_000)
      const check = () => {
        const found = frames.find((frame) => frame.at > promptAt && /"type":"part\.(delta|updated)"/.test(frame.text) && frame.text.includes('"ok"'))
        if (!found) return setTimeout(check, 1)
        clearTimeout(deadline)
        resolve(found.at)
      }
      check()
    })
    socket.close()
    return { coldStartMs, promptToProviderMs: stub.requestAt() - promptAt, providerToEventMs: textAt - stub.requestAt(), systemChars: stub.systemChars(), toolsChars: stub.toolsChars() }
  } finally {
    proc.kill()
    await proc.exited
    stub.stop()
    for (const dir of [home, data, workspace]) rmSync(dir, { recursive: true, force: true })
  }
}

Bun.spawnSync(["cargo", "build", "-q", "--release", "-p", "drift-engined"], { cwd: root, stdout: "inherit", stderr: "inherit" })
const results: Result[] = []
for (let run = 0; run < runs; run += 1) results.push(await bench())
const pick = (key: keyof Result) => Math.round(median(results.map((result) => result[key])))
const system = pick("systemChars")
const tools = pick("toolsChars")
console.log(JSON.stringify({ runs, coldStartMs: pick("coldStartMs"), promptToProviderMs: pick("promptToProviderMs"), providerToEventMs: pick("providerToEventMs"), systemChars: system, toolsChars: tools, approxTokensPerTurn: Math.round((system + tools) / 4) }, null, 2))
