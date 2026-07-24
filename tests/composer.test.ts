import { expect, test } from "bun:test"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

test("composer drafts are isolated by session and new-workspace scope", async () => {
  const { clearComposerDraft, composerDraft, composerScope, patchComposerDraft } = await import("../src/state/composer")
  const session = composerScope("s1", "w1")
  const other = composerScope("s2", "w1")
  const fresh = composerScope(null, "w1")
  patchComposerDraft(session, { text: "session one" })
  patchComposerDraft(fresh, { text: "new thread" })
  expect(composerDraft(session).text).toBe("session one")
  expect(composerDraft(other).text).toBe("")
  expect(composerDraft(fresh).text).toBe("new thread")
  clearComposerDraft(session)
  clearComposerDraft(fresh)
})

test("reverted messages restore uploads and file mentions", async () => {
  const { draftFromMessage } = await import("../src/state/composer")
  const restored = draftFromMessage({
    info: { id: "u1", sessionID: "s1", role: "user" },
    parts: [
      { id: "t1", type: "text", text: "check @src/app.tsx", messageID: "u1", sessionID: "s1" },
      {
        id: "i1",
        type: "file",
        filename: "screen.png",
        mime: "image/png",
        url: "data:image/png;base64,abc",
        messageID: "u1",
        sessionID: "s1",
      },
      {
        id: "p1",
        type: "file",
        filename: "notes.pdf",
        mime: "application/pdf",
        url: "data:application/pdf;base64,def",
        messageID: "u1",
        sessionID: "s1",
      },
      {
        id: "f1",
        type: "file",
        filename: "app.tsx",
        mime: "text/plain",
        url: "file:///C:/work/src/app.tsx",
        source: { type: "file", path: "C:/work/src/app.tsx", text: { value: "@src/app.tsx", start: 6, end: 18 } },
        messageID: "u1",
        sessionID: "s1",
      },
    ],
  } as never)
  expect(restored.text).toBe("check @src/app.tsx")
  expect(restored.staged.map((file) => [file.filename, file.mime, file.dataUrl])).toEqual([
    ["screen.png", "image/png", "data:image/png;base64,abc"],
    ["notes.pdf", "application/pdf", "data:application/pdf;base64,def"],
  ])
  expect(restored.mentions).toEqual(["src/app.tsx"])
})

test("workspace collapse IDs toggle without losing other workspaces", async () => {
  const { nextCollapsedWorkspaceIds } = await import("../src/state/workspaces")
  expect(nextCollapsedWorkspaceIds(["w1"], "w2")).toEqual(["w1", "w2"])
  expect(nextCollapsedWorkspaceIds(["w1", "w2"], "w1")).toEqual(["w2"])
})

test("model manager defaults to OpenCode's newest recent model per provider family", async () => {
  const { defaultVisibleModelIds } = await import("../src/ui/model-manager")
  const now = Date.UTC(2026, 6, 22)
  const visible = defaultVisibleModelIds(
    [
      { id: "xai/grok-old", label: "Grok old", providerID: "xai", family: "grok", releaseDate: "2026-04-01" },
      { id: "xai/grok-new", label: "Grok new", providerID: "xai", family: "grok", releaseDate: "2026-06-01" },
      { id: "xai/code", label: "Code", providerID: "xai", family: "code", releaseDate: "2026-05-01" },
      { id: "other/grok", label: "Other Grok", providerID: "other", family: "grok", releaseDate: "2026-05-15" },
      { id: "xai/legacy", label: "Legacy", providerID: "xai", family: "legacy", releaseDate: "2025-01-01" },
      { id: "xai/undated", label: "Undated", providerID: "xai", family: "unknown" },
    ],
    now,
  )
  expect([...visible]).toEqual(["xai/undated", "xai/grok-new", "xai/code", "other/grok"])
})

test("model manager orders enabled models first and preserves provider rearrangement", async () => {
  const { mergeModelProviderOrder, reorderModelProviderIds } = await import("../src/state/prefs")
  const { sortManagerModelItems } = await import("../src/ui/model-manager")
  expect(mergeModelProviderOrder(["nvidia", "xai"], ["xai", "openai", "nvidia"])).toEqual([
    "nvidia",
    "xai",
    "openai",
  ])
  expect(reorderModelProviderIds(["nvidia", "xai", "openai"], "openai", "nvidia")).toEqual([
    "openai",
    "nvidia",
    "xai",
  ])
  expect(reorderModelProviderIds(["nvidia", "xai", "openai"], "nvidia", "openai")).toEqual([
    "xai",
    "nvidia",
    "openai",
  ])
  expect(reorderModelProviderIds(["nvidia", "xai", "openai"], "nvidia", null)).toEqual([
    "xai",
    "openai",
    "nvidia",
  ])
  const items = [
    { id: "z", label: "Zulu" },
    { id: "b", label: "Beta" },
    { id: "a", label: "Alpha" },
  ]
  expect(sortManagerModelItems(items, (item) => item.id !== "b").map((item) => item.id)).toEqual(["a", "z", "b"])
})

test("shell transcript preserves a visible command-output gap and normalizes output", async () => {
  const { shellAtBottom, shellScrollTarget, shellTranscript } = await import("../src/ui/parts")
  expect(shellTranscript("bun run build", "\u001b[32mok\u001b[0m\r\ndone")).toBe("$ bun run build\n\nok\ndone")
  expect(shellAtBottom(300, 200, 501)).toBe(true)
  expect(shellAtBottom(250, 200, 501)).toBe(false)
  expect(shellScrollTarget(250, false, 700)).toBe(250)
  expect(shellScrollTarget(300, true, 700)).toBe(700)
})

test("question drafts preserve single, multiple, and custom answers", async () => {
  const { questionAnswer, selectQuestionCustom, selectQuestionOption } = await import("../src/ui/attention")
  const empty = { selected: [], custom: "", customSelected: false }
  const single = selectQuestionOption(empty, "Desktop app", false)
  expect(questionAnswer(single)).toEqual(["Desktop app"])
  expect(selectQuestionOption(single, "Web app", false)).toEqual({
    selected: ["Web app"],
    custom: "",
    customSelected: false,
  })
  const multiple = selectQuestionOption(selectQuestionOption(empty, "Tests", true), "Docs", true)
  expect(questionAnswer(multiple)).toEqual(["Tests", "Docs"])
  expect(questionAnswer(selectQuestionOption(multiple, "Tests", true))).toEqual(["Docs"])
  expect(questionAnswer({ ...selectQuestionCustom(multiple, true), custom: "  Benchmarks  " })).toEqual([
    "Tests",
    "Docs",
    "Benchmarks",
  ])
  expect(selectQuestionCustom(single, false).selected).toEqual([])
})
