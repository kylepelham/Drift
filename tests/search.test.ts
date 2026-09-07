import { expect, test } from "bun:test"
import type { MessageEntry } from "../src/engine/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined, removeItem: () => undefined },
  })

const session = (id: string, title: string, directory: string, updatedAt: number) => ({
  id,
  title,
  directory,
  updatedAt,
})
const workspace = (id: string, name: string, path: string) => ({ id, name, path })

test("title search ranks literal matches first and resolves the owning workspace", async () => {
  const { searchSessionNames } = await import("../src/state/session-search")
  const sessions = [
    session("a", "Vulkan renderer notes", "C:\\work\\app", 10),
    session("b", "Rewrite the vulkan swapchain", "C:/work/app", 30),
    session("c", "Talking about revulkanization", "C:\\work\\app", 40),
    session("d", "Unrelated", "C:\\work\\app", 50),
  ]
  const workspaces = [workspace("w1", "App", "C:\\work\\app")]

  const hits = searchSessionNames(sessions, workspaces, new Set(["b"]), "vulkan")
  // Prefix match, then word-boundary match, then a match buried inside another word.
  expect(hits.map((hit) => hit.sessionId)).toEqual(["a", "b", "c"])
  expect(hits[0].workspaceName).toBe("App")
  // Slash direction and case must not stop a session mapping to its workspace.
  expect(hits[1].workspaceName).toBe("App")
  expect(hits[1].archived).toBeTrue()
  expect(hits[0].archived).toBeFalse()
})

test("title search ignores queries too short to be useful", async () => {
  const { searchSessionNames, sessionSearchReady } = await import("../src/state/session-search")
  const sessions = [session("a", "anything", "C:/work", 1)]
  expect(sessionSearchReady("a")).toBeFalse()
  expect(sessionSearchReady(" ab ")).toBeTrue()
  expect(searchSessionNames(sessions, [], new Set(), "a")).toEqual([])
})

test("content search keeps only the newest answer when queries overtake each other", async () => {
  const { createSessionSearchRunner } = await import("../src/state/session-search")
  let release!: () => void
  const blocked = new Promise<void>((resolve) => {
    release = resolve
  })
  const runner = createSessionSearchRunner({
    store: {
      searchSessions: async (query: string) => {
        if (query === "slow") await blocked
        return [
          {
            sessionId: query,
            messageId: "m1",
            title: `${query} thread`,
            directory: "C:/work",
            updatedAt: 5,
            excerpt: `about ${query}`,
          },
        ]
      },
    },
    sessions: () => [],
    workspaces: () => [workspace("w1", "Work", "C:/work")],
    archived: () => new Set(),
  })

  const applied: string[] = []
  const apply = (state: { hits?: { sessionId: string }[] }) => {
    if (state.hits) applied.push(state.hits.map((hit) => hit.sessionId).join(",") || "empty")
  }

  const slow = runner("slow", "content", apply)
  const fast = runner("fast", "content", apply)
  await fast
  release()
  await slow

  // The superseded query resolves last but must not overwrite the newer result.
  expect(applied).toEqual(["fast"])
})

test("content hits map to workspaces and carry the matching message", async () => {
  const { contentHits } = await import("../src/state/session-search")
  const hits = contentHits(
    [
      {
        sessionId: "s1",
        messageId: "m9",
        title: "Old thread",
        directory: "C:\\work\\app",
        updatedAt: 7,
        excerpt: "…the vulkan swapchain…",
      },
      { sessionId: "s2", messageId: "", title: "Orphan", directory: "D:\\gone", updatedAt: 3, excerpt: "x" },
    ],
    [workspace("w1", "App", "C:/work/app")],
    new Set(["s1"]),
  )
  expect(hits[0]).toMatchObject({ workspaceId: "w1", workspaceName: "App", messageId: "m9", archived: true })
  // A workspace Drift no longer tracks still lists, labelled by its directory.
  expect(hits[1]).toMatchObject({ workspaceId: "", workspaceName: "D:\\gone", messageId: undefined })
})

test("excerpt highlighting splits every occurrence without building markup", async () => {
  const { highlightSegments } = await import("../src/state/session-search")
  expect(highlightSegments("The Vulkan and vulkan bits", "vulkan")).toEqual([
    { text: "The ", match: false },
    { text: "Vulkan", match: true },
    { text: " and ", match: false },
    { text: "vulkan", match: true },
    { text: " bits", match: false },
  ])
  expect(highlightSegments("nothing", "")).toEqual([{ text: "nothing", match: false }])
})

const entry = (id: string, parts: unknown[]): MessageEntry =>
  ({ info: { id, sessionID: "s1", role: "assistant", time: { created: 1 } }, parts }) as MessageEntry

test("in-session search reads every visible surface and counts repeats", async () => {
  const { entrySearchText, transcriptMatches, totalMatches } = await import("../src/state/transcript-search")
  const entries = [
    entry("m1", [{ type: "text", text: "the cache is warm" }]),
    entry("m2", [
      { type: "reasoning", text: "the cache might be cold" },
      { type: "tool", state: { output: "cache miss, cache miss" } },
    ]),
    entry("m3", [{ type: "text", text: "unrelated" }]),
    // Drift's own scaffolding is not something the user ever saw.
    entry("m4", [{ type: "text", text: "cache", synthetic: true }]),
  ]

  const matches = transcriptMatches(entries, "cache")
  expect(matches).toEqual([
    { messageId: "m1", count: 1 },
    { messageId: "m2", count: 3 },
  ])
  expect(totalMatches(matches)).toBe(4)
  expect(entrySearchText(entries[3])).toBe("")
  expect(transcriptMatches(entries, "  ")).toEqual([])
})

test("match navigation wraps at both ends and survives a growing transcript", async () => {
  const { occurrenceAt, reanchorMatch, stepMatch } = await import("../src/state/transcript-search")
  expect(stepMatch(0, 3, 1)).toBe(1)
  expect(stepMatch(2, 3, 1)).toBe(0)
  expect(stepMatch(0, 3, -1)).toBe(2)
  expect(stepMatch(0, 0, 1)).toBe(-1)

  // The cursor walks occurrences: repeats inside one message are distinct stops.
  const matches = [
    { messageId: "m2", count: 2 },
    { messageId: "m5", count: 1 },
  ]
  expect(occurrenceAt(matches, 0)).toEqual({ messageId: "m2", index: 0 })
  expect(occurrenceAt(matches, 1)).toEqual({ messageId: "m2", index: 1 })
  expect(occurrenceAt(matches, 2)).toEqual({ messageId: "m5", index: 0 })
  expect(occurrenceAt(matches, 3)).toBeUndefined()
  expect(occurrenceAt(matches, -1)).toBeUndefined()

  // An older page loading in front of the cursor must not move the highlight to another message.
  const after = [{ messageId: "m1", count: 3 }, ...matches]
  expect(reanchorMatch(after, { messageId: "m5", index: 0 }, 0)).toBe(5)
  // The second occurrence in a message stays the second occurrence after the set grows.
  expect(reanchorMatch(after, { messageId: "m2", index: 1 }, 0)).toBe(4)
  // An occurrence that disappeared clamps to the last one that still exists in its message.
  expect(reanchorMatch(after, { messageId: "m2", index: 9 }, 0)).toBe(4)
  // A message that stopped matching falls back to the nearest position that still exists.
  expect(reanchorMatch(after, { messageId: "gone", index: 0 }, 9)).toBe(5)
  expect(reanchorMatch([], { messageId: "m5", index: 0 }, 0)).toBe(-1)
})

test("the search text cache follows a message that is still streaming", async () => {
  const { transcriptMatches } = await import("../src/state/transcript-search")
  const streaming = entry("m1", [{ type: "text", text: "cache" }])
  expect(transcriptMatches([streaming], "cache")).toEqual([{ messageId: "m1", count: 1 }])
  // The same object growing (a streamed reply) must invalidate the cached lowered text.
  ;(streaming.parts[0] as { text: string }).text = "cache then cache again"
  expect(transcriptMatches([streaming], "cache")).toEqual([{ messageId: "m1", count: 2 }])
})
