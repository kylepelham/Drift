import { expect, test } from "bun:test"
import type { Event } from "@opencode-ai/sdk/client"
import { reduce } from "../src/engine/events"
import { createEngineState } from "../src/engine/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

const toolEvent = (partId: string, tool: string, status: string): Event =>
  ({
    type: "message.part.updated",
    properties: {
      part: { id: partId, sessionID: "child", messageID: "m1", type: "tool", tool, state: { status } },
    },
  }) as unknown as Event

test("session.updated clears revert and share keys the engine dropped", () => {
  const [state, set] = createEngineState()
  const updated = (info: Record<string, unknown>): Event =>
    ({ type: "session.updated", properties: { info } }) as unknown as Event
  reduce(set, updated({ id: "s1", title: "t", revert: { messageID: "m5" }, share: { url: "u" } }))
  expect(state.sessions["s1"].revert?.messageID).toBe("m5")
  reduce(set, updated({ id: "s1", title: "t" }))
  expect(state.sessions["s1"].revert).toBeUndefined()
  expect(state.sessions["s1"].share).toBeUndefined()
})

test("fixEscapedEmphasis lets path-ending emphasis close without touching escapes or code", async () => {
  const { fixEscapedEmphasis } = await import("../src/ui/markdown")
  expect(fixEscapedEmphasis("*C:\\* (30 entries)")).toBe("*C:\\\\* (30 entries)")
  expect(fixEscapedEmphasis("**S:\\Personal\\Drift\\** done")).toBe("**S:\\Personal\\Drift\\\\** done")
  expect(fixEscapedEmphasis("`**C:\\**` and ```\nS:\\**\n```")).toBe("`**C:\\**` and ```\nS:\\**\n```")
  expect(fixEscapedEmphasis("literal \\*star\\* stays and 5 \\* 3")).toBe("literal \\*star\\* stays and 5 \\* 3")
})

test("markdown escapes the unclosed HTML tag that enlarged the rest of a stored response", async () => {
  const { prepareMarkdown } = await import("../src/ui/markdown")
  const response =
    'contains the game title in an <h1 class="post-title">. A 404 won\'t.\n\nLet me check the lengths.'
  expect(prepareMarkdown(response)).toBe(
    'contains the game title in an &lt;h1 class="post-title"&gt;. A 404 won\'t.\n\nLet me check the lengths.',
  )
})

test("markdown preserves balanced, void, and code-fenced HTML", async () => {
  const { prepareMarkdown } = await import("../src/ui/markdown")
  expect(prepareMarkdown("<details><summary>More</summary>Text</details><br>")).toBe(
    "<details><summary>More</summary>Text</details><br>",
  )
  expect(prepareMarkdown("`<h1>`\n```html\n<h2>Example</h2>\n```")).toBe(
    "`<h1>`\n```html\n<h2>Example</h2>\n```",
  )
  expect(prepareMarkdown("orphan </strong> text")).toBe("orphan &lt;/strong&gt; text")
})

test("markdown preserves tilde fences and multi-backtick code spans", async () => {
  const { prepareMarkdown } = await import("../src/ui/markdown")
  expect(prepareMarkdown("~~~html\n<h1>Example\n~~~\nafter")).toBe("~~~html\n<h1>Example\n~~~\nafter")
  expect(prepareMarkdown("Use ``<h1>`literal`</h1>`` here")).toBe("Use ``<h1>`literal`</h1>`` here")
})

test("streaming tables bound incomplete links and preserve completed anchors", async () => {
  const { prepareMarkdown } = await import("../src/ui/markdown")
  const { marked } = await import("marked")
  const url = `https://example.com/${"long-segment".repeat(20)}`
  const prefix = "| Resource | State |\n| --- | --- |\n"
  const incomplete = marked.parse(prepareMarkdown(`${prefix}| [documentation](${url} | loading |`), { async: false })
  const complete = marked.parse(prepareMarkdown(`${prefix}| [documentation](${url}) | ready |`), { async: false })
  expect(incomplete).toContain("<table>")
  expect(incomplete).toContain(url)
  expect(incomplete).toContain(`>${url}</a>`)
  expect(complete).toContain(`<a href="${url}">documentation</a>`)

  const css = await Bun.file(new URL("../src/styles/app.css", import.meta.url)).text()
  expect(css).toMatch(/\.md :where\(table\) \{[^}]*width: 100%;[^}]*table-layout: fixed;/s)
  expect(css).toMatch(/\.md :where\(th, td\) \{[^}]*overflow-wrap: anywhere;/s)
})

test("user markdown preserves literal Windows path backslashes", async () => {
  const { prepareMarkdown } = await import("../src/ui/markdown")
  expect(prepareMarkdown("Open \\\\server\\share\\folder", true)).toBe(
    "Open &#92;&#92;server&#92;share&#92;folder",
  )
  expect(prepareMarkdown("`\\\\server\\share` and ```text\nC:\\work\n```", true)).toBe(
    "`\\\\server\\share` and ```text\nC:\\work\n```",
  )
})

test("human-typed prose keeps accidental block markers literal", async () => {
  const { prepareMarkdown } = await import("../src/ui/markdown")
  expect(prepareMarkdown("> quoted", true)).toBe("&gt; quoted")
  expect(prepareMarkdown(">> continued", true)).toBe("&gt;> continued")
  expect(prepareMarkdown("# comment", true)).toBe("&#35; comment")
  expect(prepareMarkdown("#!/bin/sh", true)).toBe("#!/bin/sh")
  expect(prepareMarkdown("-----", true)).toBe("&#45;----")
  expect(prepareMarkdown("=====", true)).toBe("&#61;====")
  expect(prepareMarkdown("snake_case_name and *glob*", true)).toBe("snake&#95;case&#95;name and &#42;glob&#42;")
  expect(prepareMarkdown("~~kept~~", true)).toBe("&#126;&#126;kept&#126;&#126;")
})

test("human-typed prose renders shell transcripts and separators as written", async () => {
  const { prepareMarkdown } = await import("../src/ui/markdown")
  const { marked } = await import("marked")
  const url = "https://example.test/api/a_b?id=42"
  const transcript = [`PS C:\\Demo> probe ${url}`, ">> retrying with diagnostic headers", `403 ${url}`].join("\n")
  const html = marked.parse(prepareMarkdown(transcript, true), { async: false })
  expect(html).not.toContain("<blockquote>")
  expect(html).toContain(`href="${url}"`)

  const notes = marked.parse(prepareMarkdown("Deployment notes\n----------------\nRestart it.", true), {
    async: false,
  })
  expect(notes).not.toContain("<hr")
  expect(notes).not.toContain("<h1")
  expect(notes).not.toContain("<h2")
  expect(notes).toContain("Deployment notes")
})

test("human-typed prose keeps pasted markup literal", async () => {
  const { prepareMarkdown } = await import("../src/ui/markdown")
  const { marked } = await import("marked")
  const paste = "<configuration>\n  <system.webServer>\n    <rewrite />\n  </system.webServer>\n</configuration>"
  const html = marked.parse(prepareMarkdown(paste, true), { async: false })
  expect(html).not.toMatch(/<(configuration|system\.webServer|rewrite)/)
  expect(html).toContain("&lt;configuration")
  const fenced = "```xml\n<configuration />\n```"
  expect(prepareMarkdown(fenced, true)).toBe(fenced)

  const css = await Bun.file(new URL("../src/styles/app.css", import.meta.url)).text()
  expect(css).not.toMatch(/\.user-paste \{[^}]*max-height:/s)
  expect(css).toMatch(/\.transcript-scroll \{[^}]*overflow-anchor: none/s)
})

test("human-typed prose still renders deliberate fences and tables", async () => {
  const { prepareMarkdown } = await import("../src/ui/markdown")
  const { marked } = await import("marked")
  const fenced = "```powershell\n> $value = 1\n-----\n```"
  expect(prepareMarkdown(fenced, true)).toBe(fenced)
  const table = marked.parse(prepareMarkdown("| Name | State |\n| --- | --- |\n| a_b | ok |", true), { async: false })
  expect(table).toContain("<table>")
  expect(table).toContain("<td>a&#95;b</td>")
})

test("generated user-role seed prompts keep full markdown", async () => {
  const { prepareMarkdown } = await import("../src/ui/markdown")
  const seed = "## Carried context\nUse the *active* summary."
  expect(prepareMarkdown(seed)).toBe(seed)
})

test("progressive code chunks retain the complete file", async () => {
  const { codeChunks } = await import("../src/ui/markdown")
  const code = Array.from({ length: 401 }, (_, index) => `line ${index + 1}`).join("\n")
  expect(codeChunks(code).length).toBe(3)
  expect(codeChunks(code).join("\n")).toBe(code)
})

test("diff parsing does not invent a context row for the trailing newline", async () => {
  const { parseDiff } = await import("../src/ui/parts")
  expect(parseDiff("@@ -4,1 +4,1 @@\n-old\n+new\n")).toEqual([
    { kind: "del", line: 4, text: "old" },
    { kind: "add", line: 4, text: "new" },
  ])
})

test("diff parsing distinguishes file headers from source lines and separates hunks", async () => {
  const { parseDiff } = await import("../src/ui/parts")
  const diff = [
    "diff --git a/file b/file",
    "--- a/file",
    "+++ b/file",
    "@@ -1,2 +1,2 @@",
    "---flag",
    "+++flag",
    " keep",
    "--- a/other",
    "+++ b/other",
    "@@ -10 +10 @@",
    "-old",
    "+new",
    "",
  ].join("\n")
  expect(parseDiff(diff)).toEqual([
    { kind: "del", line: 1, text: "--flag" },
    { kind: "add", line: 1, text: "++flag" },
    { kind: "ctx", line: 2, text: "keep" },
    { kind: "gap", text: "" },
    { kind: "del", line: 10, text: "old" },
    { kind: "add", line: 10, text: "new" },
  ])
})

test("Shiki promise caches evict by approximate size and retry failures", async () => {
  const { AsyncSizeCache } = await import("../src/ui/markdown")
  const cache = new AsyncSizeCache<string>(10, (value) => value.length)
  await cache.set("first", 3, Promise.resolve("1234"))
  await cache.set("second", 3, Promise.resolve("5678"))
  expect(cache.size).toBeLessThanOrEqual(10)
  expect(cache.count).toBe(1)
  expect(cache.get("first")).toBeUndefined()
  expect(await cache.get("second")).toBe("5678")

  const failure = cache.set("failure", 1, Promise.reject(new Error("highlight failed")))
  await failure.catch(() => undefined)
  expect(cache.get("failure")).toBeUndefined()
  cache.set("oversized", 11, Promise.resolve("x"))
  expect(cache.get("oversized")).toBeUndefined()
})

test("taskBody extracts prompt and task_result for task cards", async () => {
  const { taskBody } = await import("../src/ui/parts")
  const part = (tool: string, input: Record<string, string>, output: string) =>
    ({ tool, state: { status: "completed", input, output } }) as never
  expect(taskBody(part("task", { prompt: "do x" }, "<task id=\"s1\" state=\"completed\">\n<task_result>\nall done\n</task_result>\n</task>"))).toEqual({
    prompt: "do x",
    result: "all done",
  })
  expect(taskBody(part("spawn_thread", { task: "spin off" }, "Spawned thread ok"))).toEqual({
    prompt: "spin off",
    result: "Spawned thread ok",
  })
  expect(taskBody(part("bash", {}, "x"))).toBeNull()
})

test("task headings retain the agent and task title", async () => {
  const { taskHeading } = await import("../src/ui/parts")
  expect(taskHeading("explore", "Map settings translations")).toBe("Explore Map settings translations")
  expect(taskHeading("general")).toBe("General")
})

test("compaction-only user messages retain their delimiter part", async () => {
  const { compactionParts } = await import("../src/ui/message")
  const entry = {
    info: { id: "m1", role: "user", sessionID: "s1" },
    parts: [{ id: "p1", messageID: "m1", sessionID: "s1", type: "compaction", auto: true }],
  } as never
  expect(compactionParts(entry).map((part) => part.id)).toEqual(["p1"])
})

test("streamed tool replacements retain mounted group and plugin identities", async () => {
  const { groupParts, updatePartGroupSlots } = await import("../src/ui/message")
  // Bun selects Solid's server condition for tests, so load the browser primitives
  // used by Vite to verify the keyed mount behavior without requiring a DOM.
  // @ts-expect-error Solid's browser build shares the package's public types.
  const { createRoot, createSignal, mapArray, onCleanup } = await import("solid-js/dist/solid.js") as typeof import("solid-js")
  // @ts-expect-error Solid's browser store build shares the package's public types.
  const { createStore, reconcile } = await import("solid-js/store/dist/store.js") as typeof import("solid-js/store")
  const createSlot = (group: ReturnType<typeof groupParts>[number]) => {
    const [value, setValue] = createStore(group)
    return { id: group.id, value, update: (updated: typeof group) => setValue(reconcile(updated)) }
  }
  const tool = (id: string, name: string, status: string, output = "") => ({
    id,
    sessionID: "s1",
    messageID: "m1",
    type: "tool",
    tool: name,
    state: status === "completed"
      ? { status, input: {}, output }
      : { status, input: {}, metadata: { output } },
  })

  createRoot((dispose) => {
    const slots = new Map()
    const initial = updatePartGroupSlots(groupParts([
      tool("shell", "bash", "running", "first"),
      tool("plugin", "custom-stream", "running", "one"),
      tool("read-1", "read", "running"),
      tool("read-2", "read", "running"),
    ] as never), slots, createSlot)
    const [groups, setGroups] = createSignal(initial)
    let mounts = 0
    let cleanups = 0
    const mounted = mapArray(
      groups,
      (slot) => {
        mounts++
        const state = { scrollTop: 37, following: false, pluginRevision: 4 }
        onCleanup(() => cleanups++)
        return { slot, state }
      },
    )
    const first = mounted()
    const firstById = new Map(first.map((item) => [item.slot.id, item]))
    const explored = firstById.get("explored:read-1")!.slot.value
    const firstExplored = "explored" in explored ? [...explored.explored] : []
    expect(mounts).toBe(3)

    setGroups(updatePartGroupSlots(groupParts([
      { id: "text", sessionID: "s1", messageID: "m1", type: "text", text: "Now visible" },
      tool("plugin", "custom-stream", "completed", "two"),
      tool("shell", "bash", "running", "first\nsecond"),
      tool("read-0", "read", "completed"),
      tool("read-1", "read", "completed"),
      tool("read-2", "read", "completed"),
    ] as never), slots, createSlot))
    const updated = mounted()
    const updatedById = new Map(updated.map((item) => [item.slot.id, item]))
    expect(updated.map((item) => item.slot.id)).toEqual(["text", "plugin", "shell", "explored:read-1"])
    expect(updatedById.get("shell")).toBe(firstById.get("shell"))
    expect(updatedById.get("plugin")).toBe(firstById.get("plugin"))
    expect(updatedById.get("explored:read-1")).toBe(firstById.get("explored:read-1"))
    expect(updatedById.get("shell")!.state).toEqual({ scrollTop: 37, following: false, pluginRevision: 4 })
    expect(updatedById.get("plugin")!.state.pluginRevision).toBe(4)
    const updatedExplored = updatedById.get("explored:read-1")!.slot.value
    expect("explored" in updatedExplored && updatedExplored.explored.map((part) => part.id)).toEqual([
      "read-0",
      "read-1",
      "read-2",
    ])
    expect("explored" in updatedExplored && updatedExplored.explored[1]).toBe(firstExplored[0])
    expect("explored" in updatedExplored && updatedExplored.explored[2]).toBe(firstExplored[1])
    expect(mounts).toBe(4)
    expect(cleanups).toBe(0)

    setGroups(updatePartGroupSlots(groupParts([
      tool("read-2", "read", "completed"),
      { id: "divider", sessionID: "s1", messageID: "m1", type: "text", text: "Split" },
      tool("read-0", "read", "completed"),
      tool("read-1", "read", "completed"),
    ] as never), slots, createSlot))
    const split = mounted()
    const splitExplored = split.filter((item) => "explored" in item.slot.value)
    expect(splitExplored.map((item) => item.slot.id)).toEqual(["explored:read-2", "explored:read-1"])
    expect(splitExplored[0]).not.toBe(firstById.get("explored:read-1"))
    expect(splitExplored[1]).toBe(firstById.get("explored:read-1"))

    setGroups(updatePartGroupSlots(groupParts([
      tool("read-2", "read", "completed"),
      tool("read-0", "read", "completed"),
      tool("read-1", "read", "completed"),
    ] as never), slots, createSlot))
    const merged = mounted()
    expect(merged).toHaveLength(1)
    expect(merged[0]).toBe(splitExplored[0])
    expect(merged[0].slot.id).toBe("explored:read-2")
    dispose()
    expect(cleanups).toBe(mounts)
  })
})

test("compaction boundary merges into its adjacent summary", async () => {
  const { mergeCompactionEntries } = await import("../src/ui/chat")
  const boundary = {
    info: { id: "u1", role: "user", sessionID: "s1" },
    parts: [{ id: "p1", messageID: "u1", sessionID: "s1", type: "compaction", auto: true }],
  }
  const summary = {
    info: { id: "a1", role: "assistant", sessionID: "s1", parentID: "u1", summary: true },
    parts: [{ id: "p2", messageID: "a1", sessionID: "s1", type: "text", text: "summary" }],
  }
  expect(mergeCompactionEntries([boundary, summary] as never).map((entry) => entry.info.id)).toEqual(["a1"])
  expect(mergeCompactionEntries([boundary] as never).map((entry) => entry.info.id)).toEqual(["u1"])
})

test("successful compaction clears a transient session error", () => {
  const [state, set] = createEngineState()
  set("errors", "s1", "Your input exceeds the context window")
  reduce(
    set,
    { type: "session.compacted", properties: { sessionID: "s1" } } as unknown as Event,
  )
  expect(state.errors["s1"]).toBeUndefined()
})

test("sidebar drag converts screen movement through the current zoom scale", async () => {
  const { sidebarWidthFromDrag } = await import("../src/ui/sidebar")
  expect(sidebarWidthFromDrag(256, 30, 1.5)).toBe(276)
  expect(sidebarWidthFromDrag(470, 30, 1)).toBe(480)
})

test("fixed menus convert visual coordinates and viewport bounds through CSS zoom", async () => {
  const { fixedMenuPosition } = await import("../src/state/zoom")
  const metrics = { scale: 1.5, viewportWidth: 1200, viewportHeight: 900 }
  expect(fixedMenuPosition(300, 225, 200, 100, metrics)).toEqual({ left: 200, top: 150, viewportHeight: 600 })
  expect(fixedMenuPosition(1170, 870, 200, 100, metrics)).toEqual({ left: 592, top: 492, viewportHeight: 600 })
})

test("provider credentials dispose cached instances and refresh connection state", async () => {
  const { createActions } = await import("../src/engine/actions")
  const [state, set] = createEngineState()
  set("directory", "C:\\repo")
  const requests: string[] = []
  const providerLists = [["opencode"], []]
  const client = {
    auth: {
      set: async () => ({ data: true }),
    },
    provider: {
      list: async () => ({
        data: { all: [], connected: providerLists.shift() ?? [], default: {} },
      }),
    },
  }
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init)
    requests.push(`${request.method} ${new URL(request.url).pathname}`)
    return new Response("true", { status: 200, headers: { "content-type": "application/json" } })
  }) as typeof fetch

  try {
    const actions = createActions(
      () => client as never,
      state,
      set,
      () => ({ url: "http://engine.test" }),
    )
    expect(await actions.setProviderKey("opencode", "test-key")).toEqual({ ok: true, connected: true })
    expect(state.connected).toEqual(["opencode"])
    expect(await actions.disconnectProvider("opencode")).toEqual({ ok: true, connected: false })
    expect(state.connected).toEqual([])
    expect(await actions.reloadProviders()).toBe(true)
    expect(requests).toEqual([
      "POST /global/dispose",
      "DELETE /auth/opencode",
      "POST /global/dispose",
      "POST /global/dispose",
    ])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("embedded engine connections use the shell password with the opencode user", async () => {
  const previous = (globalThis as { __TAURI__?: unknown }).__TAURI__
  ;(globalThis as { __TAURI__?: unknown }).__TAURI__ = {
    core: { invoke: async () => ({ url: "http://127.0.0.1:4321", error: null, password: "sidecar-secret" }) },
  }
  try {
    const { resolveEngine } = await import("../src/engine/connection")
    expect(await resolveEngine()).toEqual({
      url: "http://127.0.0.1:4321",
      headers: { Authorization: `Basic ${btoa("opencode:sidecar-secret")}` },
    })
  } finally {
    ;(globalThis as { __TAURI__?: unknown }).__TAURI__ = previous
  }
})

test("MCP engine actions propagate transport and SDK failures", async () => {
  const { createActions } = await import("../src/engine/actions")
  const [state, set] = createEngineState()
  const client = {
    mcp: {
      status: async (): Promise<unknown> => ({ error: { message: "status rejected" } }),
      connect: async () => ({ error: { message: "connect rejected" } }),
      disconnect: async () => ({ data: false }),
      auth: { authenticate: async () => ({ error: { data: { message: "auth rejected" } } }) },
    },
  }
  const actions = createActions(() => client as never, state, set, () => ({ url: "http://engine.test" }))
  await expect(actions.mcpStatus()).rejects.toThrow("status rejected")
  await expect(actions.mcpConnect("docs")).rejects.toThrow("connect rejected")
  await expect(actions.mcpDisconnect("docs")).rejects.toThrow("Could not disconnect docs")
  await expect(actions.mcpAuthenticate("docs")).rejects.toThrow("auth rejected")
  client.mcp.status = async () => { throw new Error("network down") }
  await expect(actions.mcpStatus()).rejects.toThrow("network down")
})

test("upward transcript gestures unstick immediately near the bottom", async () => {
  const { accumulatedWheelTarget, normalizedWheelDelta, scrollGestureSticks, shouldShowScrollToBottom } = await import(
    "../src/ui/chat"
  )
  expect(scrollGestureSticks(1000, 980, 20)).toBeFalse()
  expect(scrollGestureSticks(980, 1000, 20)).toBeTrue()
  expect(scrollGestureSticks(980, 1000, 120)).toBeFalse()
  expect(shouldShowScrollToBottom(79)).toBeFalse()
  expect(shouldShowScrollToBottom(80)).toBeTrue()
  expect(normalizedWheelDelta(3, 0, 800)).toBe(3)
  expect(normalizedWheelDelta(3, 1, 800)).toBe(48)
  expect(normalizedWheelDelta(2, 2, 800)).toBe(1600)
  expect(accumulatedWheelTarget(100, null, 40, 500)).toBe(140)
  expect(accumulatedWheelTarget(105, 140, 40, 500)).toBe(180)
  expect(accumulatedWheelTarget(490, null, 40, 500)).toBe(500)
})

test("transcript follow revision tracks lengths and status without embedding large output", async () => {
  const { transcriptRevision } = await import("../src/ui/chat")
  const part = (output: string, status = "running") => ({
    parts: [{ type: "tool", state: { status, metadata: { output } } }],
  })
  const first = transcriptRevision(part("a".repeat(1_550_000)))
  const sameLength = transcriptRevision(part("b".repeat(1_550_000)))
  const completed = transcriptRevision(part("b".repeat(1_550_000), "completed"))
  expect(first).toBe(sameLength)
  expect(first).not.toContain("aaaa")
  expect(first.length).toBeLessThan(64)
  expect(completed).not.toBe(first)
})

test("timeline omits hidden-only messages without dropping the active thinking row", async () => {
  const { timelineEntries } = await import("../src/ui/chat")
  const entry = (id: string, parts: unknown[]) => ({
    info: { id, role: "assistant", time: { created: 1 }, tokens: { input: 0, output: 0, reasoning: 0 } },
    parts,
  })
  const hidden = entry("hidden", [{ id: "r1", type: "reasoning", text: "private", time: { start: 1, end: 2 } }])
  const todo = entry("todo", [{ id: "t1", type: "tool", tool: "todowrite", state: { status: "completed" } }])
  const visible = entry("visible", [{ id: "t2", type: "tool", tool: "edit", state: { status: "completed" } }])

  expect(timelineEntries([hidden, todo, visible] as never).map((item) => item.info.id)).toEqual(["visible"])
  expect(timelineEntries([hidden, todo, visible] as never, "hidden").map((item) => item.info.id)).toEqual([
    "hidden",
    "visible",
  ])
})

test("tall row measurement only compensates rows actually above the viewport", async () => {
  const { resizeCompensation } = await import("../src/ui/chat")
  expect(resizeCompensation(96, 2000, 2100, 1000)).toBe(0)
  expect(resizeCompensation(96, 2000, 900, 1000)).toBe(1904)
})

test("virtual range clamps a stale scroll offset after a tall row collapses", async () => {
  const { virtualRange } = await import("../src/ui/chat")
  expect(virtualRange([0, 100, 450, 550], 5000, 800)).toEqual({ start: 0, end: 3 })
  expect(virtualRange([0, 500, 1000, 1096], 5000, 400)).toEqual({ start: 0, end: 3 })
})

test("large multiline user content uses a full-height literal row estimate", async () => {
  const { largeUserText } = await import("../src/ui/message")
  expect(largeUserText("x".repeat(1999))).toBeFalse()
  expect(largeUserText("x".repeat(2000))).toBeTrue()
  expect(largeUserText(Array.from({ length: 40 }, () => "line").join("\n"))).toBeFalse()
  expect(largeUserText(Array.from({ length: 41 }, () => "line").join("\n"))).toBeTrue()

  const css = await Bun.file(new URL("../src/styles/app.css", import.meta.url)).text()
  expect(css).toMatch(/\.user-paste \{[^}]*white-space: pre/s)
  expect(css).toMatch(/\.user-paste \{[^}]*overflow-x: auto/s)
  expect(css).toMatch(/\.user-paste \{[^}]*overflow-y: hidden/s)

  const { estimatedTimelineRow } = await import("../src/ui/chat")
  const entry = (text: string, generated = false) => ({
    info: { id: "u1", role: "user", time: { created: 1 } },
    parts: [{ type: "text", text, metadata: generated ? { generated: true } : undefined }],
  }) as never
  const long = Array.from({ length: 41 }, () => "line").join("\n")
  expect(estimatedTimelineRow(entry(long))).toBe(915)
  expect(estimatedTimelineRow(entry(long), 16)).toBe(1112)
  expect(estimatedTimelineRow(entry(long, true))).toBe(967)
})

test("assistant row estimates account for wrapping and fenced code", async () => {
  const { estimatedTimelineRow, estimateTextLines } = await import("../src/ui/chat")
  expect(estimateTextLines("a".repeat(176), 88)).toBe(2)
  expect(estimateTextLines("```text\n" + "a".repeat(176) + "\n```", 88)).toBe(3)
  const entry = {
    info: { id: "a1", role: "assistant", time: { created: 1 } },
    parts: [{ type: "text", text: "a".repeat(849) }],
  } as never
  expect(estimatedTimelineRow(entry)).toBe(272)
})

test("thinking follows OpenCode's active user turn", async () => {
  const { thinkingAfterMessage } = await import("../src/ui/chat")
  const message = (id: string, role: "user" | "assistant", parentID?: string, completed?: number) =>
    ({ info: { id, role, parentID, time: { created: 1, completed } }, parts: [] }) as never
  const first = message("u1", "user")
  const response = message("a1", "assistant", "u1")
  const steer = message("u2", "user")
  expect(thinkingAfterMessage([first, response, steer], "busy")).toBe("a1")
  response.info.time.completed = 2
  expect(thinkingAfterMessage([first, response, steer], "busy")).toBe("u2")
  const steeredResponse = message("a2", "assistant", "u2")
  expect(thinkingAfterMessage([first, response, steer, steeredResponse], "busy")).toBe("a2")
  expect(thinkingAfterMessage([first, response, steer, steeredResponse], "retry")).toBe("a2")
  expect(thinkingAfterMessage([first, response, steer, steeredResponse], "idle")).toBeNull()
})

test("thinking derives the first provider reasoning heading for the active turn", async () => {
  const { reasoningHeading, thinkingState } = await import("../src/ui/chat")
  expect(reasoningHeading("## Inspecting `events.ts` ##\n\nChecking the reducer.")).toBe("Inspecting events.ts")
  expect(reasoningHeading("<h3>Comparing <em>providers</em></h3>")).toBe("Comparing providers")
  expect(reasoningHeading("**Reading [OpenCode](https://opencode.ai) behavior**\n\nDetails")).toBe("Reading OpenCode behavior")
  expect(reasoningHeading("Unformatted reasoning text")).toBeUndefined()

  const user = { info: { id: "u1", role: "user", time: { created: 1 } }, parts: [] }
  const assistant = {
    info: { id: "a1", role: "assistant", parentID: "u1", time: { created: 2 } },
    parts: [{ id: "p1", type: "reasoning", text: "**Tracing session state**", time: { start: 2 } }],
  }
  expect(thinkingState([user, assistant] as never, "busy")).toEqual({
    messageID: "a1",
    heading: "Tracing session state",
  })
})

test("retry presentation follows OpenCode countdown and truncation", async () => {
  const { retryPresentation } = await import("../src/ui/chat")
  const status = { type: "retry", attempt: 3, message: "x".repeat(90), next: 15_000 } as const
  expect(retryPresentation(status, 7_400)).toEqual({
    message: "x".repeat(80) + "...",
    truncated: true,
    info: "Retrying in 8s - attempt #3",
  })
  expect(retryPresentation({ ...status, message: "Rate limited" }, 16_000).info).toBe("Retrying - attempt #3")
})

test("busy thinking is suppressed by an assistant error while retry remains visible", async () => {
  const { thinkingState } = await import("../src/ui/chat")
  const entries = [
    { info: { id: "u1", role: "user", time: { created: 1 } }, parts: [] },
    {
      info: {
        id: "a1",
        role: "assistant",
        parentID: "u1",
        time: { created: 2 },
        error: { name: "APIError", data: { message: "failed" } },
      },
      parts: [],
    },
  ] as never
  expect(thinkingState(entries, "busy")).toBeNull()
  expect(thinkingState(entries, "retry")?.messageID).toBe("a1")
})

test("assistant errors unwrap provider JSON and preserve plain text", async () => {
  const { errorText, unwrapErrorMessage } = await import("../src/engine/error")
  expect(unwrapErrorMessage('Error: {"error":{"type":"rate_limit","message":"slow down"}}')).toBe(
    "rate_limit: slow down",
  )
  expect(unwrapErrorMessage('prefix {"message":"credit balance is too low"} suffix')).toBe(
    "credit balance is too low",
  )
  expect(errorText({ name: "ProviderError", data: { message: "plain failure" } })).toBe("plain failure")
})

test("message part deltas accumulate streamed reasoning summaries", () => {
  const [state, set] = createEngineState()
  set("loaded", "s1", true)
  set("transcripts", "s1", [
    {
      info: { id: "a1", sessionID: "s1", role: "assistant", time: { created: 1 } },
      parts: [{ id: "p1", sessionID: "s1", messageID: "a1", type: "reasoning", text: "**Tracing" }],
    },
  ] as never)
  reduce(
    set,
    {
      type: "message.part.delta",
      properties: { sessionID: "s1", messageID: "a1", partID: "p1", field: "text", delta: " events**" },
    } as never,
  )
  expect((state.transcripts.s1[0].parts[0] as { text: string }).text).toBe("**Tracing events**")
})

test("context usage skips a trailing zero-token assistant message", async () => {
  const { contextStats } = await import("../src/engine/store")
  const [state, set] = createEngineState()
  const assistant = (id: string, total: number) => ({
    info: {
      id,
      sessionID: "s1",
      role: "assistant",
      providerID: "openai",
      modelID: "gpt-5",
      tokens: { total, input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
    },
    parts: [],
  })
  set("transcripts", "s1", [assistant("a1", 50_000), assistant("a2", 0)] as never)
  set("providers", [
    {
      id: "openai",
      name: "OpenAI",
      models: { "gpt-5": { id: "gpt-5", limit: { context: 100_000 } } },
    },
  ] as never)
  expect(contextStats(state, "s1")?.count).toBe(50_000)
  expect(contextStats(state, "s1")?.percent).toBe(50)
})

test("activity counts distinct tool parts and tracks the running tool", () => {
  const [state, set] = createEngineState()
  reduce(set, toolEvent("p1", "grep", "running"))
  reduce(set, toolEvent("p1", "grep", "completed"))
  reduce(set, toolEvent("p2", "read", "pending"))
  reduce(set, toolEvent("p2", "read", "running"))
  expect(state.activity["child"].tools).toBe(2)
  expect(state.activity["child"].current).toBe("read")
  reduce(set, toolEvent("p2", "read", "completed"))
  expect(state.activity["child"].tools).toBe(2)
  expect(state.activity["child"].current).toBeUndefined()
})

test("session errors terminate busy activity and remain visible", () => {
  const [state, set] = createEngineState()
  set("status", "s1", { type: "busy" })
  set("activity", "s1", { tools: 1, lastPartId: "p1", current: "bash" })
  reduce(
    set,
    {
      type: "session.error",
      properties: { sessionID: "s1", error: { name: "ProviderError", data: { message: "credit balance is too low" } } },
    } as never,
  )
  expect(state.status["s1"].type).toBe("idle")
  expect(state.activity["s1"].current).toBeUndefined()
  expect(state.errors["s1"]).toBe("credit balance is too low")
})

test("current ask events update immediately and retain their workspace directory", () => {
  const [state, set] = createEngineState()
  reduce(
    set,
    {
      type: "permission.asked",
      properties: {
        id: "perm-1",
        sessionID: "s1",
        permission: "bash",
        patterns: ["git status"],
        metadata: { title: "Run command" },
        tool: { messageID: "m1", callID: "c1" },
      },
    } as never,
    "C:/repo",
  )
  reduce(
    set,
    {
      type: "question.asked",
      properties: { id: "q1", sessionID: "s1", questions: [{ question: "Continue?", header: "Continue", options: [] }] },
    } as never,
    "C:/repo",
  )
  expect(state.permissions.s1[0]).toMatchObject({
    id: "perm-1",
    type: "bash",
    pattern: ["git status"],
    title: "Run command",
    metadata: { directory: "C:/repo" },
  })
  expect(state.questions.s1[0].directory).toBe("C:/repo")

  reduce(set, { type: "permission.replied", properties: { sessionID: "s1", requestID: "perm-1" } } as never)
  reduce(set, { type: "question.rejected", properties: { sessionID: "s1", requestID: "q1" } } as never)
  expect(state.permissions.s1).toEqual([])
  expect(state.questions.s1).toEqual([])
})

test("toast and sessionless error events become visible notices", () => {
  const [state, set] = createEngineState()
  reduce(
    set,
    {
      id: "toast-1",
      type: "tui.toast.show",
      properties: { title: "Connected", message: "Provider ready", variant: "success", duration: 2500 },
    } as never,
  )
  reduce(set, { type: "session.error", properties: {} } as never)
  expect(state.notices[0]).toMatchObject({
    id: "toast-1",
    title: "Connected",
    message: "Provider ready",
    variant: "success",
    duration: 2500,
  })
  expect(state.notices[1]).toMatchObject({ title: "Drift error", message: "An error occurred", variant: "error" })
})

test("identical runtime errors collapse into one visible notice", () => {
  const [state, set] = createEngineState()
  const toast = (id: string) =>
    reduce(
      set,
      {
        id,
        type: "tui.toast.show",
        properties: { title: "Drift error", message: "Failed to load plugin", variant: "error" },
      } as never,
    )
  toast("error-1")
  toast("error-2")
  toast("error-3")
  expect(state.notices).toHaveLength(1)
  expect(state.notices[0].id).toBe("error-3")
})

test("a new active status clears stale fallback errors", () => {
  const [state, set] = createEngineState()
  set("errors", "s1", "old failure")
  reduce(set, { type: "session.status", properties: { sessionID: "s1", status: { type: "busy" } } } as never)
  expect(state.errors.s1).toBeUndefined()
})
