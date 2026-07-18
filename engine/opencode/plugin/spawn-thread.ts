import { tool, type Plugin } from "@opencode-ai/plugin"

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

        const history = await client.session.messages({ path: { id: ctx.sessionID }, query: { directory } })
        const lastAssistant = history.data?.findLast((entry) => entry.info.role === "assistant")?.info
        const model =
          lastAssistant && "modelID" in lastAssistant
            ? { providerID: lastAssistant.providerID, modelID: lastAssistant.modelID }
            : undefined

        const seed = [
          "You are starting a thread that was spawned from another conversation. The context below was carried over for you.",
          `## Carried context\n${args.summary}`,
          args.context ? `## Excerpts\n${args.context}` : "",
          `## Task\n${args.task}`,
        ]
          .filter(Boolean)
          .join("\n\n")

        await client.session.promptAsync({
          path: { id: session.id },
          body: { parts: [{ type: "text", text: seed }], model, agent: ctx.agent },
          query: { directory },
        })

        return {
          title: `Spawned: ${args.title}`,
          output: [
            `Spawned thread "${args.title}" (id ${session.id}) and it is now working on the task.`,
            "The user can open it from the sidebar and continue that conversation directly.",
            "Do not repeat the spawned task here; report back to the user that the thread was spawned.",
          ].join(" "),
          metadata: { sessionId: session.id, spawned: true },
        }
      },
    }),
  },
})
