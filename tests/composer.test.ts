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

test("composer clipboard publishing uses the exact selected text", async () => {
  const { composerSelection } = await import("../src/ui/composer")
  expect(composerSelection("first\nsecond\nthird", 6, 12)).toBe("second")
  expect(composerSelection("abcdef", 5, 2)).toBe("cde")
  expect(composerSelection("abcdef", 3, 3)).toBe("")
})

test("slash parsing preserves command argument mode", async () => {
  const { parseSlash, slashPresets } = await import("../src/ui/slash")
  expect(parseSlash("/fork")).toEqual({ query: "fork", args: "", separated: false })
  expect(parseSlash("/fork all")).toEqual({ query: "fork", args: "all", separated: true })
  expect(parseSlash("/spawn Investigate auth failures")).toEqual({
    query: "spawn",
    args: "Investigate auth failures",
    separated: true,
  })
  expect(parseSlash("//fork")).toBeNull()
  expect(
    slashPresets(
      {
        name: "fork",
        description: "fork",
        presets: [{ value: "all", label: "all", description: "all", execute: true }],
      },
      "a",
    ).map((item) => item.value),
  ).toEqual(["all"])
})

test("composer history is normalized, deduplicated, and bounded", async () => {
  const { maxComposerHistory, normalizeComposerHistory, prependComposerHistory } = await import("../src/state/composer")
  const draft = { text: "  carry on  ", staged: [], mentions: ["src/app.tsx", "src/app.tsx"] }
  const first = prependComposerHistory([], draft)
  expect(first).toEqual([{ text: "carry on", mentions: ["src/app.tsx"] }])
  expect(prependComposerHistory(first, draft)).toBe(first)
  const entries = Array.from({ length: maxComposerHistory }, (_, index) => ({ text: `prompt ${index}`, mentions: [] }))
  const bounded = prependComposerHistory(entries, { text: "newest", staged: [], mentions: [] })
  expect(bounded).toHaveLength(maxComposerHistory)
  expect(bounded[0].text).toBe("newest")
  expect(bounded.at(-1)?.text).toBe("prompt 98")
  expect(
    normalizeComposerHistory([
      null,
      { text: "  valid  ", mentions: ["a", 1, "a"] },
      { text: "" },
      { nope: true },
    ]),
  ).toEqual([{ text: "valid", mentions: ["a"] }])
})

test("composer history navigation restores the draft and attachments", async () => {
  const { canNavigateComposerHistory, navigateComposerHistory } = await import("../src/state/composer")
  const entries = [
    { text: "newest", mentions: ["new.ts"] },
    { text: "oldest", mentions: [] },
  ]
  const current = {
    text: "",
    staged: [{ id: "f1", filename: "screen.png", mime: "image/png", dataUrl: "data:image/png;base64,x", size: 1 }],
    mentions: ["draft.ts"],
  }
  const newest = navigateComposerHistory(entries, { index: -1, saved: null }, current, "up")
  expect(newest?.draft).toEqual({ text: "newest", staged: [], mentions: ["new.ts"] })
  expect(newest?.cursor).toBe("start")
  if (!newest) throw new Error("expected history navigation")
  const oldest = navigateComposerHistory(entries, newest.navigation, newest.draft, "up")
  expect(oldest?.draft.text).toBe("oldest")
  if (!oldest) throw new Error("expected older history entry")
  expect(navigateComposerHistory(entries, oldest.navigation, oldest.draft, "up")).toBeUndefined()
  const forward = navigateComposerHistory(entries, oldest.navigation, oldest.draft, "down")
  expect(forward?.draft.text).toBe("newest")
  if (!forward) throw new Error("expected newer history entry")
  const restored = navigateComposerHistory(entries, forward.navigation, forward.draft, "down")
  expect(restored?.draft).toEqual(current)
  expect(restored?.navigation).toEqual({ index: -1, saved: null })
  expect(canNavigateComposerHistory("up", "", 0, false)).toBeTrue()
  expect(canNavigateComposerHistory("up", "draft", 0, false)).toBeFalse()
  expect(canNavigateComposerHistory("down", "draft", 5, true)).toBeTrue()
  expect(canNavigateComposerHistory("down", "draft", 2, true)).toBeFalse()
})

test("drag reorder ignores released pointers and converts zoomed geometry", async () => {
  const { dragLayoutScale, dragPointerPressed, dragReorderAllowed } = await import("../src/ui/drag-reorder")
  expect(dragPointerPressed(4, { pointerId: 4, buttons: 1 })).toBeTrue()
  expect(dragPointerPressed(4, { pointerId: 4, buttons: 0 })).toBeFalse()
  expect(dragPointerPressed(4, { pointerId: 5, buttons: 1 })).toBeFalse()
  expect(dragLayoutScale(52, 40)).toBe(1.3)
  expect(dragLayoutScale(32, 40)).toBe(0.8)
  expect(dragLayoutScale(0, 0)).toBe(1)
  expect(dragReorderAllowed({ button: 0, isPrimary: true, pointerType: "mouse" })).toBeTrue()
  expect(dragReorderAllowed({ button: 0, isPrimary: true, pointerType: "touch" })).toBeFalse()
  expect(dragReorderAllowed({ button: 0, isPrimary: false, pointerType: "pen" })).toBeFalse()
})

test("file drags gate the drop target without child churn or text-selection drags", async () => {
  const { dragHasFiles, dropTargetActive, nextDragDepth, splitDroppedFiles } = await import("../src/ui/drag-drop")
  expect(dragHasFiles(["Files"])).toBeTrue()
  expect(dragHasFiles(["text/plain", "text/uri-list"])).toBeFalse()
  expect(dragHasFiles(undefined)).toBeFalse()
  let depth = nextDragDepth(0, "enter")
  depth = nextDragDepth(depth, "enter") // child dragenter fires before the parent dragleave
  depth = nextDragDepth(depth, "leave")
  expect(dropTargetActive(depth)).toBeTrue()
  depth = nextDragDepth(depth, "leave")
  expect(dropTargetActive(depth)).toBeFalse()
  expect(nextDragDepth(0, "leave")).toBe(0)
  expect(nextDragDepth(3, "drop")).toBe(0)
  expect(nextDragDepth(2, "end")).toBe(0)

  const item = (name: string, directory: boolean) => ({
    kind: "file",
    getAsFile: () => ({ name }) as File,
    webkitGetAsEntry: () => ({ isDirectory: directory }),
  })
  const text = { kind: "string", getAsFile: () => null }
  const split = splitDroppedFiles([item("a.png", false), item("src", true), text, item("b.csv", false)])
  expect(split.files.map((file) => file.name)).toEqual(["a.png", "b.csv"])
  expect(split.directories).toBe(1)
  const fallbackFile = { name: "pasted.txt" } as File
  expect(splitDroppedFiles([], [fallbackFile]).files).toEqual([fallbackFile])
  expect(splitDroppedFiles([item("dir", true)], [fallbackFile])).toEqual({ files: [], directories: 1 })
})

test("dropped OS files reach the same staging pipeline as the picker", async () => {
  const composer = await Bun.file("src/ui/composer.tsx").text()
  expect(composer).toContain('window.addEventListener("dragenter", onDragEnter)')
  expect(composer).toContain('window.addEventListener("drop", onDrop)')
  expect(composer).toContain("void addFiles(dropped.files)")
  expect(composer).toContain("drift.composer.dropFiles")
  expect(composer).toContain("drift.composer.folderUnsupported")
  // Tauri must not intercept native drops, or WebView2 never fires HTML5 drop with DataTransfer files.
  const conf = JSON.parse(await Bun.file("src-tauri/tauri.conf.json").text())
  expect(conf.app.windows[0].dragDropEnabled).toBeFalse()
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
  const {
    createFrameCoalescer,
    createShellTranscriptStream,
    initialToolOpen,
    initialToolOpenForPart,
    rememberToolOpen,
    shellAtBottom,
    shellReplaceSegments,
    shellScrollTarget,
    shellTranscript,
    shellTimeoutStatus,
  } = await import("../src/ui/parts")
  expect(shellTranscript("bun run build", "\u001b[32mok\u001b[0m\r\ndone")).toBe("$ bun run build\n\nok\ndone")
  const output = Array.from({ length: 10_000 }, (_, index) => `line ${index}`).join("\r\n")
  const transcript = shellTranscript("generate", output)
  expect(transcript.startsWith("$ generate\n\nline 0\nline 1")).toBeTrue()
  expect(transcript.endsWith("line 9999")).toBeTrue()
  expect(initialToolOpen("bash", "completed", false)).toBeTrue()
  expect(initialToolOpen("bash", "running", false)).toBeTrue()
  expect(initialToolOpen("read", "completed", true)).toBeFalse()
  expect(initialToolOpen("bash", "error", true)).toBeTrue()
  expect(initialToolOpen("bash", "error", false)).toBeFalse()
  const expansionPartId = `expansion-${Date.now()}`
  expect(initialToolOpenForPart(expansionPartId, "edit", "completed", false)).toBeFalse()
  rememberToolOpen(expansionPartId, true)
  expect(initialToolOpenForPart(expansionPartId, "edit", "completed", false)).toBeTrue()
  rememberToolOpen(expansionPartId, false)
  expect(initialToolOpenForPart(expansionPartId, "bash", "running", false)).toBeFalse()
  expect(shellAtBottom(300, 200, 501)).toBe(true)
  expect(shellAtBottom(250, 200, 501)).toBe(false)
  expect(shellScrollTarget(250, false, 700)).toBe(250)
  expect(shellScrollTarget(300, true, 700)).toBe(700)
  const shellPart = (status: "running" | "completed", metadata: Record<string, unknown>) =>
    ({ tool: "bash", state: { status, input: {}, metadata } }) as never
  expect(shellTimeoutStatus(shellPart("running", { shellTimeoutMs: 300_000 }))).toEqual({
    timedOut: false,
    text: "Limit 5m",
  })
  expect(shellTimeoutStatus(shellPart("running", { shellTimeoutMs: null }))).toBeNull()
  expect(shellTimeoutStatus(shellPart("completed", { shellTimeoutMs: 60_000, timedOut: true }))).toEqual({
    timedOut: true,
    text: "Timed out after 1m",
  })

  const stream = createShellTranscriptStream()
  expect(stream.update("generate", "\u001b[3", false)).toEqual({ replace: true, text: "$ generate" })
  expect(stream.update("generate", "\u001b[32mok\r", false)).toEqual({ replace: true, text: "$ generate\n\nok" })
  expect(stream.update("generate", "\u001b[32mok\r\ndone", false)).toEqual({ replace: false, text: "\ndone" })
  expect(stream.update("generate", "\u001b[32mok\r\ndone", true)).toEqual({
    replace: true,
    text: "$ generate\n\nok\ndone",
  })
  const sliding = createShellTranscriptStream()
  expect(sliding.update("tail", "line one", false)).toEqual({ replace: true, text: "$ tail\n\nline one" })
  expect(sliding.update("tail", "line two", false)).toEqual({ replace: true, text: "$ tail\n\nline two" })
  expect(sliding.update("tail", "...\n\nnew sliding preview", false)).toEqual({
    replace: true,
    text: "$ tail\n\n...\n\nnew sliding preview",
  })
  const boundary = createShellTranscriptStream()
  const beforePreview = "x".repeat(30_000)
  boundary.update("tail", beforePreview, false)
  const previewUpdate = boundary.update("tail", `...\n\n${"x".repeat(29_995)}abcde`, false)
  expect(previewUpdate.replace).toBeTrue()
  expect(previewUpdate.text.startsWith("$ tail\n\n...\n\n")).toBeTrue()
  const whitespace = createShellTranscriptStream()
  expect(whitespace.update("wait", "\n ", false)).toEqual({ replace: true, text: "$ wait" })
  expect(whitespace.update("wait", "\n ready", false)).toEqual({ replace: true, text: "$ wait\n\n\n ready" })

  // Replace frames split into a styled command line plus trailing output; unexpected frames fall
  // back to verbatim output so no text is ever dropped.
  expect(shellReplaceSegments("bun run build", "$ bun run build\n\nok\ndone")).toEqual({
    command: "bun run build",
    output: "\n\nok\ndone",
  })
  expect(shellReplaceSegments("wait", "$ wait")).toEqual({ command: "wait", output: "" })
  expect(shellReplaceSegments("other", "$ mismatch\n\ntext")).toEqual({ command: null, output: "$ mismatch\n\ntext" })

  const frames = new Map<number, () => void>()
  const values: string[] = []
  let handle = 0
  const coalescer = createFrameCoalescer<string>(
    (callback) => {
      frames.set(++handle, callback)
      return handle
    },
    (id) => frames.delete(id),
    (value) => values.push(value),
  )
  coalescer.push("first", true)
  coalescer.push("latest", true)
  expect(frames.size).toBe(1)
  expect(values).toEqual([])
  const callback = frames.get(handle)
  frames.delete(handle)
  callback?.()
  expect(values).toEqual(["latest"])
  coalescer.push("pending", true)
  coalescer.push("completed", false)
  expect(values).toEqual(["latest", "completed"])
  expect(frames.size).toBe(0)
  coalescer.dispose()
})

test("active tool rows keep their target subtitle visible", async () => {
  const source = await Bun.file("src/ui/parts.tsx").text()
  expect(source).toContain('info().subtitle && !(props.part.tool === "bash" && expanded())')
  expect(source).not.toContain("info().subtitle && !active()")
})

test("delegated tool headers always toggle while the arrow owns navigation", async () => {
  const { activateToolHeader, openSpawnedThread, toolChevronVisible } = await import("../src/ui/parts")
  const toggled: string[] = []
  for (const lifecycle of ["pending-no-child", "running-with-child", "completed"])
    activateToolHeader(() => toggled.push(lifecycle))
  expect(toggled).toEqual(["pending-no-child", "running-with-child", "completed"])

  let stopped = false
  let selected = ""
  openSpawnedThread({ stopPropagation: () => (stopped = true) }, "child", (id) => (selected = id))
  expect(stopped).toBeTrue()
  expect(selected).toBe("child")
  expect(toggled).toHaveLength(3)
  expect(toolChevronVisible(true, true)).toBeTrue()
  expect(toolChevronVisible(true, false)).toBeFalse()
  expect(toolChevronVisible(false, false)).toBeTrue()
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

test("auto-approved permissions never become the visible manual request", async () => {
  const { firstManualPermission } = await import("../src/ui/composer")
  const permission = (id: string, sessionID: string) => ({ id, sessionID }) as never
  const permissions = [permission("auto", "s1"), permission("manual", "s2")]
  expect(firstManualPermission(permissions, (item) => item.sessionID === "s1")?.id).toBe("manual")
  expect(firstManualPermission([permissions[0]], () => true)).toBeUndefined()
})

test("queued questions retain focus while other requests arrive or reorder", async () => {
  const { focusedQuestion } = await import("../src/ui/composer")
  const question = (id: string) => ({ id }) as never
  const first = question("q1")
  const second = question("q2")
  expect(focusedQuestion([first, second])?.id).toBe("q1")
  expect(focusedQuestion([second, first], "q1")?.id).toBe("q1")
  expect(focusedQuestion([second], "q1")?.id).toBe("q2")
})

test("composer attention cards share one stack without suppressing concurrent requests", async () => {
  const app = await Bun.file("src/app.tsx").text()
  const composer = await Bun.file("src/ui/composer.tsx").text()
  const attention = await Bun.file("src/ui/attention.tsx").text()
  const revert = await Bun.file("src/ui/revert-dock.tsx").text()
  const css = await Bun.file("src/styles/app.css").text()

  expect(app).not.toContain("<AttentionStrip />")
  expect(composer).toContain('class="composer-attention-stack')
  expect(composer).toContain("<AttentionStrip />")
  expect(composer).toContain("<Show keyed when={pendingQuestion()?.id}>")
  expect(composer).toContain("<Show when={pendingAsk()}>")
  expect(composer.match(/class="flow-root"/g)).toHaveLength(3)
  expect(composer).not.toContain("pendingPermission() ? undefined : pendingQuestion()")
  expect(composer).not.toContain("pendingPermission() || pendingQuestion() ? undefined : pendingAsk()")
  expect(attention.match(/composer-layer-card/g)).toHaveLength(3)
  expect(revert).toContain("composer-layer-card")
  expect(css).not.toContain(".composer-attention-stack:has(> :nth-child(2))")
  expect(css).not.toMatch(/\.composer-attention-stack[^}]*overflow/s)
})

test("question steps and answers persist independently by request id", async () => {
  const { clearQuestionDraft, questionDraftState, setQuestionDraftStep, updateQuestionDraft } = await import(
    "../src/state/question-drafts"
  )
  clearQuestionDraft("q1")
  clearQuestionDraft("q2")

  updateQuestionDraft("q1", 2, 0, { selected: ["Tests"], custom: "", customSelected: false })
  setQuestionDraftStep("q1", 2, 1)
  updateQuestionDraft("q1", 2, 1, { selected: [], custom: "Benchmarks", customSelected: true })
  updateQuestionDraft("q2", 1, 0, { selected: ["Docs"], custom: "", customSelected: false })

  expect(questionDraftState("q1", 2)).toEqual({
    step: 1,
    drafts: [
      { selected: ["Tests"], custom: "", customSelected: false },
      { selected: [], custom: "Benchmarks", customSelected: true },
    ],
  })
  expect(questionDraftState("q2", 1).drafts[0].selected).toEqual(["Docs"])
  expect(questionDraftState("q1", 2).step).toBe(1)

  clearQuestionDraft("q1")
  clearQuestionDraft("q2")
})

test("confirmed question events clear the matching persisted draft", async () => {
  const { reduce } = await import("../src/engine/events")
  const { createEngineState } = await import("../src/engine/store")
  const { questionDraftState, updateQuestionDraft } = await import("../src/state/question-drafts")
  const [state, set] = createEngineState()
  const asked = {
    type: "question.asked",
    properties: { id: "q-event", sessionID: "s1", questions: [] },
  } as never
  reduce(set, asked)
  updateQuestionDraft("q-event", 1, 0, { selected: ["Keep me"], custom: "", customSelected: false })
  reduce(set, {
    type: "question.replied",
    properties: { requestID: "q-event", sessionID: "s1" },
  } as never)
  expect(state.questions.s1).toEqual([])
  expect(questionDraftState("q-event", 1).drafts[0].selected).toEqual([])
})
