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

test("shell transcript preserves a visible command-output gap and normalizes output", async () => {
  const { shellTranscript } = await import("../src/ui/parts")
  expect(shellTranscript("bun run build", "\u001b[32mok\u001b[0m\r\ndone")).toBe("$ bun run build\n\nok\ndone")
})
