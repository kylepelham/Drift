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

test("a revert older than the loaded page backfills instead of blanking the transcript", async () => {
  const { revertBackfillNeeded } = await import("../src/ui/chat")
  // Real shape from a 35k-message session: the marker sat 325 messages back while only the
  // newest 100 were loaded, so every loaded row was inside the reverted range and the timeline
  // rendered empty. Backfill must run until a pre-revert row survives the filter.
  expect(revertBackfillNeeded({ revertedAt: "msg_marker", visible: 0, loaded: true, cursor: "older" })).toBeTrue()
  // Stops as soon as anything is visible.
  expect(revertBackfillNeeded({ revertedAt: "msg_marker", visible: 1, loaded: true, cursor: "older" })).toBeFalse()
  // Never fires for a session without a revert, mid-load, or with history exhausted.
  expect(revertBackfillNeeded({ visible: 0, loaded: true, cursor: "older" })).toBeFalse()
  expect(revertBackfillNeeded({ revertedAt: "msg_marker", visible: 0, cursor: "older" })).toBeFalse()
  expect(revertBackfillNeeded({ revertedAt: "msg_marker", visible: 0, loaded: true, cursor: null })).toBeFalse()
})

test("a failed backfill page is not requested again until the cursor moves", async () => {
  const { revertBackfillAttempt } = await import("../src/ui/chat")
  // A page that never arrived leaves the cursor in place, so the retry gate has to key on it:
  // matching the last failure means asking again would repeat the request that just failed.
  expect(revertBackfillAttempt("ses_one", "older")).toBe(revertBackfillAttempt("ses_one", "older"))
  expect(revertBackfillAttempt("ses_one", "older")).not.toBe(revertBackfillAttempt("ses_one", "older-still"))
  expect(revertBackfillAttempt("ses_one", "older")).not.toBe(revertBackfillAttempt("ses_two", "older"))
  // A missing cursor must not collide with a session whose id ends where the separator would be.
  expect(revertBackfillAttempt("ses_one")).not.toBe(revertBackfillAttempt("ses_one", "older"))
  expect(revertBackfillAttempt("ses_one", null)).toBe(revertBackfillAttempt("ses_one"))

  const source = await Bun.file("src/ui/chat.tsx").text()
  expect(source).toContain("if (revertBackfillFailure() === attempt) return")
  expect(source).toContain("if (!loaded) setRevertBackfillFailure(attempt)")
})

test("retry models come from connected providers once the engine is online", async () => {
  const { retryModelItems } = await import("../src/ui/chat")
  const { createEngineState } = await import("../src/engine/store")
  const model = (id: string) => ({ id, name: id, capabilities: { toolcall: true }, limit: { context: 200_000 } })
  const [state, set] = createEngineState()
  set("providers", [
    { id: "anthropic", name: "Anthropic", models: { fable: model("fable") } },
    { id: "openai", name: "OpenAI", models: { "gpt-5": model("gpt-5") } },
  ] as never)

  // Before the first listing there is nothing to filter against, so retrying can offer anything.
  set("connection", "connecting")
  expect(retryModelItems(state).map((item) => item.id)).toEqual(["anthropic/fable", "openai/gpt-5"])

  // Online, an empty connected list is the answer: retrying a disconnected provider only fails.
  set("connection", "online")
  expect(retryModelItems(state)).toEqual([])
  set("connected", ["openai"])
  expect(retryModelItems(state).map((item) => item.id)).toEqual(["openai/gpt-5"])
})

test("the transcript shows a loading row while reverted history backfills", async () => {
  const source = await Bun.file("src/ui/chat.tsx").text()
  // The empty-state loading row must also cover backfill, otherwise the view is blank mid-page.
  expect(source).toContain("timeline().length === 0 && (revertBackfill() ||")
  // Each finished page re-runs the effect, so paging continues past a fully reverted page.
  expect(source).toContain(".finally(() => setRevertBackfill(false))")
})
