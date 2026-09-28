import { tool, type Plugin } from "@opencode-ai/plugin"

function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message) return error.message
  if (typeof error === "string" && error.trim()) return error
  if (error && typeof error === "object") {
    const record = error as Record<string, unknown>
    for (const key of ["message", "error", "data"]) {
      const detail = errorMessage(record[key], "")
      if (detail) return detail
    }
  }
  return fallback
}

function messageID(): string {
  return `msg_${crypto.randomUUID().replaceAll("-", "")}`
}

type Client = Parameters<Plugin>[0]["client"]
type Part = {
  type: string
  text?: string
  synthetic?: boolean
  tool?: string
  state?: { status?: string; title?: string; metadata?: { sessionId?: string } }
}
type Entry = { info: { role: string; error?: unknown; time?: { created?: number; completed?: number } }; parts: Part[] }
type Todo = { content: string; status: string }
type Status = { type: string; attempt?: number; message?: string }
type Pending = { sessionID: string; permission?: string; patterns?: string[]; questions?: { question: string }[] }
type Raw = { get(options: { url: string; query: Record<string, string>; signal: AbortSignal }): Promise<{ data?: unknown; error?: unknown }> }
type Snapshot = { title: string; status: Status; pending: Pending[]; todos: Todo[]; history: Entry[] }

const replyLimit = 4000
const recentTools = 10
const todoMarks: Record<string, string> = { completed: "x", in_progress: "~", cancelled: "-" }

type Result<T> = { data?: T; error?: unknown }

async function data<T>(request: Promise<Result<T>> | Result<T>, what: string): Promise<T> {
  const response = await request
  if (response.error !== undefined || response.data === undefined) throw new Error(`Could not read ${what}: ${errorMessage(response.error, "no data")}`)
  return response.data
}

function spawnedBy(history: Entry[], id: string) {
  return history.some((entry) => entry.info.role === "assistant" && entry.parts.some((part) =>
    part.type === "tool" && part.tool === "spawn_thread" && part.state?.status === "completed" && part.state.metadata?.sessionId === id,
  ))
}

async function verifySpawn(client: Client, parentID: string, childID: string, directory: string, signal: AbortSignal) {
  const seen = new Set<string>()
  let before: string | undefined
  do {
    signal.throwIfAborted()
    const query = { directory, limit: 50, before }
    const page = await client.session.messages({ path: { id: parentID }, query, signal })
    if (spawnedBy(await data(page, "spawn receipts") as Entry[], childID)) return true
    before = page.response.headers.get("x-next-cursor") ?? undefined
    if (before && seen.has(before)) throw new Error("Could not read spawn receipts: pagination did not advance")
    if (before) seen.add(before)
  } while (before)
  return false
}

async function snapshot(client: Client, id: string, directory: string, signal: AbortSignal): Promise<Snapshot> {
  const query = { directory }
  // The v1 SDK has no pending permission/question methods; its internal client carries the engine auth.
  const raw = (client as unknown as { _client: Raw })._client
  const [session, statuses, todos, history, permissions, questions] = await Promise.all([
    data(client.session.get({ path: { id }, query, signal }), "the thread"),
    data(client.session.status({ query, signal }), "thread status") as Promise<Record<string, Status>>,
    data(client.session.todo({ path: { id }, query, signal }), "the thread todos") as Promise<Todo[]>,
    data(client.session.messages({ path: { id }, query: { ...query, limit: 50 }, signal }), "the thread messages") as Promise<Entry[]>,
    data(raw.get({ url: "/permission", query, signal }), "pending approvals") as Promise<Pending[]>,
    data(raw.get({ url: "/question", query, signal }), "pending questions") as Promise<Pending[]>,
  ])
  const pending = [...permissions, ...questions].filter((request) => request.sessionID === id)
  return { title: session.title, status: statuses[id] ?? { type: "idle" }, pending, todos, history }
}

function statusLine(view: Snapshot) {
  const waits = view.pending.map((request) => request.questions
    ? `waiting for the user to answer: ${request.questions.map((item) => item.question).join(" / ")}`
    : `waiting for the user to approve ${request.permission} ${(request.patterns ?? []).join(", ")}`.trim())
  if (waits.length) return waits.join("\n")
  if (view.status.type === "busy") return "working"
  if (view.status.type === "retry") return `retrying (attempt ${view.status.attempt}): ${view.status.message}`
  const failure = view.history.findLast((entry) => entry.info.role === "assistant")?.info.error
  return failure ? `stopped with an error: ${errorMessage(failure, "unknown error")}` : "idle"
}

function latestReply(history: Entry[]) {
  const text = history.filter((entry) => entry.info.role === "assistant")
    .map((entry) => entry.parts.filter((part) => part.type === "text" && !part.synthetic && part.text?.trim()).map((part) => part.text).join("\n"))
    .findLast(Boolean)
  if (!text) return "(no reply yet)"
  if (text.length <= replyLimit) return text
  return `${text.slice(0, replyLimit)}\n[${text.length - replyLimit} more characters; the user can open the thread for the rest]`
}

function todoList(todos: Todo[]) {
  if (!todos.length) return ""
  const lines = todos.slice(0, 20).map((todo) => `- [${todoMarks[todo.status] ?? " "}] ${todo.content.slice(0, 200)}`)
  if (todos.length > 20) lines.push(`[${todos.length - 20} more todos]`)
  return `Todos:\n${lines.join("\n")}`
}

function toolList(history: Entry[]) {
  const tools = history.flatMap((entry) => entry.parts).filter((part) => part.type === "tool").slice(-recentTools)
  if (!tools.length) return ""
  const lines = tools.map((part) => {
    const title = part.state?.title?.slice(0, 120)
    return `- ${part.tool} ${part.state?.status ?? ""}${title ? `: ${title}` : ""}`
  })
  return `Recent tool calls:\n${lines.join("\n")}`
}

function render(id: string, view: Snapshot) {
  const updated = view.history.at(-1)?.info.time
  const output = [
    `Thread "${view.title.slice(0, 200)}" (${id})`,
    `Status: ${statusLine(view).slice(0, 1000)}`,
    updated ? `Last activity: ${new Date(updated.completed ?? updated.created ?? 0).toISOString()}` : "",
    todoList(view.todos),
    toolList(view.history),
    `Latest reply:\n${latestReply(view.history)}`,
  ].filter(Boolean).join("\n\n")
  return output.length > 10000 ? `${output.slice(0, 9900)}\n[Snapshot truncated; open the thread for more]` : output
}

function readThread(client: Client) {
  return tool({
    description: [
      "Read a one-time snapshot of a thread this conversation spawned with spawn_thread:",
      "status, pending approvals, todos, recent tool calls, and its latest reply. It never waits.",
      "Only use it when the user asks to check on a spawned thread.",
      "Do not poll unless the user explicitly asks for polling.",
    ].join(" "),
    args: { id: tool.schema.string().describe("Thread id returned by spawn_thread") },
    async execute(args, ctx) {
      if (!await verifySpawn(client, ctx.sessionID, args.id, ctx.directory, ctx.abort)) {
        throw new Error(`Thread ${args.id} was not spawned from this conversation with spawn_thread.`)
      }
      const view = await snapshot(client, args.id, ctx.directory, ctx.abort)
      return { title: view.title, output: render(args.id, view), metadata: { threadId: args.id } }
    },
  })
}

export const SpawnThread: Plugin = async ({ client }) => ({
  tool: {
    read_thread: readThread(client),
    spawn_thread: tool({
      description: [
        "Spawn a new sibling chat thread for a distinct task or topic.",
        "The new thread shows up in the user's sidebar like any other thread and they can keep talking to it there.",
        "Use it when work splits off from the current conversation, or when the user asks to move something to its own thread.",
        "The new thread cannot see this conversation: carry context explicitly via the summary and excerpts arguments.",
      ].join(" "),
      args: {
        title: tool.schema.string().describe("Short title for the new thread (3-6 words)"),
        task: tool.schema.string().describe("What the new thread should do, phrased as a direct instruction"),
        summary: tool.schema.string().describe("Concise summary of the context the new thread needs"),
        context: tool.schema
          .string()
          .optional()
          .describe("Verbatim excerpts from this conversation worth carrying over word-for-word"),
      },
      async execute(args, ctx) {
        const directory = ctx.directory
        const created = await client.session.create({ body: { title: args.title }, query: { directory } })
        const session = created.data
        if (!session) return "Failed to spawn thread: session could not be created"

        const failSpawn = async (failure: string): Promise<never> => {
          try {
            const deleted = await client.session.delete({ path: { id: session.id }, query: { directory } })
            if (deleted.error !== undefined) throw deleted.error
          } catch (cleanupError) {
            throw new Error(
              `${failure} Cleanup of child session ${session.id} also failed: ${errorMessage(cleanupError, "unknown cleanup error")}`,
            )
          }
          throw new Error(failure)
        }

        let model: { providerID: string; modelID: string } | undefined
        let seed: string
        let seedMessageID: string
        try {
          const history = await client.session.messages({ path: { id: ctx.sessionID }, query: { directory } })
          if (history.error !== undefined) throw history.error
          const lastAssistant = history.data?.findLast((entry) => entry.info.role === "assistant")?.info
          model =
            lastAssistant && "modelID" in lastAssistant
              ? { providerID: lastAssistant.providerID, modelID: lastAssistant.modelID }
              : undefined
          seed = [
            "You are starting a thread that was spawned from another conversation. The context below was carried over for you.",
            `## Carried context\n${args.summary}`,
            args.context ? `## Excerpts\n${args.context}` : "",
            `## Task\n${args.task}`,
          ]
            .filter(Boolean)
            .join("\n\n")
          seedMessageID = messageID()
        } catch (error) {
          return failSpawn(
            `Failed to prepare spawned thread "${args.title}" before prompting: ${errorMessage(error, "unknown preparation error")}.`,
          )
        }

        const spawned = () => ({
          title: `Spawned: ${args.title}`,
          output: [
            `Spawned thread "${args.title}" (id ${session.id}); its seed prompt was accepted for processing.`,
            "The user can open it from the sidebar and continue that conversation directly.",
            "Do not repeat the spawned task here; report back to the user that the thread was spawned.",
          ].join(" "),
          metadata: { sessionId: session.id, spawned: true },
        })

        let prompted
        try {
          prompted = await client.session.promptAsync({
            path: { id: session.id },
            body: {
              messageID: seedMessageID,
              parts: [{ type: "text", text: seed, metadata: { generated: true } }],
              model,
              agent: ctx.agent,
            },
            query: { directory },
          })
        } catch (error) {
          const transportError = errorMessage(error, "unknown transport error")
          try {
            const admitted = await client.session.message({
              path: { id: session.id, messageID: seedMessageID },
              query: { directory },
            })
            if (admitted.data?.info.id === seedMessageID) return spawned()
          } catch {
            // The verification failure is secondary; the prompt request remains indeterminate.
          }
          throw new Error(
            `Failed to confirm whether spawned thread "${args.title}" was started after a transport error: ${transportError}. Admission is unknown and retryable; child session ${session.id} was preserved. Check for seed message ${seedMessageID} before retrying.`,
          )
        }
        if (prompted.error !== undefined) {
          return failSpawn(
            `Failed to start spawned thread "${args.title}": seed prompt was rejected: ${errorMessage(prompted.error, "unknown admission error")}.`,
          )
        }

        return spawned()
      },
    }),
  },
})
