import { expect, test } from "bun:test"
import type { Message, Part } from "@opencode-ai/sdk/client"
import type { MessageEntry } from "../src/engine/store"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

function user(id: string, text: string): MessageEntry {
  return {
    info: { id, role: "user" } as Message,
    parts: [{ id: `${id}-part`, type: "text", text } as Part],
  }
}

function assistant(id: string): MessageEntry {
  return { info: { id, role: "assistant" } as Message, parts: [] }
}

function fakeEngine(entries: MessageEntry[]) {
  const calls: string[] = []
  let marker: string | undefined
  return {
    calls,
    setMarker: (id?: string) => (marker = id),
    getMarker: () => marker,
    state: { transcripts: { s1: entries } },
    actions: {
      revert: (id: string, messageID: string) => {
        calls.push(`revert:${id}:${messageID}`)
        marker = messageID
        return Promise.resolve(true)
      },
      unrevert: (id: string) => {
        calls.push(`unrevert:${id}`)
        marker = undefined
        return Promise.resolve(true)
      },
    },
  }
}

const transcript = [user("01", "first"), assistant("02"), user("03", "second"), assistant("04"), user("05", "third")]

test("redo with no next user message unreverts and clears the draft", async () => {
  const { restoreReverted } = await import("../src/ui/revert")
  const { composerDraft, composerScope, setComposerDraft } = await import("../src/state/composer")
  const engine = fakeEngine(transcript)
  engine.setMarker("05")
  setComposerDraft(composerScope("s1"), { text: "third", staged: [], mentions: [] })
  expect(await restoreReverted(engine, "s1", "05")).toBeTrue()
  expect(engine.calls).toEqual(["unrevert:s1"])
  expect(engine.getMarker()).toBeUndefined()
  expect(composerDraft(composerScope("s1")).text).toBe("")
})

test("repeated redo advances one user turn then unreverts", async () => {
  const { restoreReverted } = await import("../src/ui/revert")
  const { composerDraft, composerScope, setComposerDraft } = await import("../src/state/composer")
  const engine = fakeEngine(transcript)
  engine.setMarker("03")
  setComposerDraft(composerScope("s1"), { text: "second", staged: [], mentions: [] })
  await restoreReverted(engine, "s1", engine.getMarker()!)
  expect(engine.calls).toEqual(["revert:s1:05"])
  expect(engine.getMarker()).toBe("05")
  expect(composerDraft(composerScope("s1")).text).toBe("third")
  await restoreReverted(engine, "s1", engine.getMarker()!)
  expect(engine.calls).toEqual(["revert:s1:05", "unrevert:s1"])
  expect(engine.getMarker()).toBeUndefined()
  expect(composerDraft(composerScope("s1")).text).toBe("")
})

test("failed revert leaves the composer draft untouched", async () => {
  const { restoreReverted } = await import("../src/ui/revert")
  const { composerDraft, composerScope, setComposerDraft } = await import("../src/state/composer")
  const engine = fakeEngine(transcript)
  engine.actions.revert = () => Promise.resolve(false)
  setComposerDraft(composerScope("s1"), { text: "kept", staged: [], mentions: [] })
  expect(await restoreReverted(engine, "s1", "03")).toBeFalse()
  expect(composerDraft(composerScope("s1")).text).toBe("kept")
})

test("failed unrevert leaves the composer draft untouched", async () => {
  const { restoreReverted } = await import("../src/ui/revert")
  const { composerDraft, composerScope, setComposerDraft } = await import("../src/state/composer")
  const engine = fakeEngine(transcript)
  engine.actions.unrevert = () => Promise.resolve(false)
  setComposerDraft(composerScope("s1"), { text: "kept", staged: [], mentions: [] })
  expect(await restoreReverted(engine, "s1", "05")).toBeFalse()
  expect(composerDraft(composerScope("s1")).text).toBe("kept")
})

test("restoring the newest undone message unreverts fully", async () => {
  const { restoreReverted, revertDockEntries } = await import("../src/ui/revert")
  const engine = fakeEngine(transcript)
  engine.setMarker("01")
  const items = revertDockEntries(transcript, engine.getMarker())
  await restoreReverted(engine, "s1", items[0].info.id)
  expect(engine.calls).toEqual(["unrevert:s1"])
})

test("restoring an earlier undone message moves the marker to the next user turn", async () => {
  const { restoreReverted } = await import("../src/ui/revert")
  const { composerDraft, composerScope } = await import("../src/state/composer")
  const engine = fakeEngine(transcript)
  engine.setMarker("01")
  await restoreReverted(engine, "s1", "03")
  expect(engine.calls).toEqual(["revert:s1:05"])
  expect(engine.getMarker()).toBe("05")
  expect(composerDraft(composerScope("s1")).text).toBe("third")
})

test("dock entries derive from the transcript and revert marker, newest first", async () => {
  const { revertDockEntries } = await import("../src/ui/revert")
  expect(revertDockEntries(transcript)).toEqual([])
  expect(revertDockEntries(transcript, "01").map((entry) => entry.info.id)).toEqual(["05", "03", "01"])
  expect(revertDockEntries(transcript, "03").map((entry) => entry.info.id)).toEqual(["05", "03"])
  expect(revertDockEntries(transcript, "05").map((entry) => entry.info.id)).toEqual(["05"])
})

test("dock previews collapse whitespace to a single line", async () => {
  const { revertPreview } = await import("../src/ui/revert")
  expect(revertPreview(user("01", "  line one\nline two  "))).toBe("line one line two")
  expect(revertPreview(undefined)).toBe("")
})
