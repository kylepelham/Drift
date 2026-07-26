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

export const SpawnThread: Plugin = async ({ client }) => ({
  tool: {
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
            body: { messageID: seedMessageID, parts: [{ type: "text", text: seed }], model, agent: ctx.agent },
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
