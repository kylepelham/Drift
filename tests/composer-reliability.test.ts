import { expect, test } from "bun:test"

if (!("localStorage" in globalThis))
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem: () => undefined },
  })

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => (resolve = done))
  return { promise, resolve }
}

const attachment = {
  id: "file-1",
  filename: "evidence.png",
  mime: "image/png",
  dataUrl: "data:image/png;base64,evidence",
  size: 8,
}

test("composer submission clears text, mentions, and files only after admission", async () => {
  const { createComposerSubmit } = await import("../src/ui/composer-submit")
  const { clearComposerDraft, composerDraft, composerScope, setComposerDraft } = await import("../src/state/composer")
  const key = composerScope("success-session", "success-workspace")
  const snapshot = { text: "  inspect this  ", mentions: ["src/app.ts"], staged: [attachment] }
  setComposerDraft(key, snapshot)
  let history: typeof snapshot | undefined

  const submit = createComposerSubmit({
    scope: () => key,
    session: () => "success-session",
    workspace: () => ({ id: "success-workspace", name: "Success", path: "C:\\success" }),
    online: () => true,
    draft: composerDraft,
    prepare: () => ({}),
    transform: async ({ text }) => text,
    newSession: async () => undefined,
    sessionScope: (id) => composerScope(id),
    migrateDraft: () => undefined,
    selectSession: () => undefined,
    sessionCreated: () => undefined,
    send: async () => ({ ok: true }),
    admitted(scope, expected, historyDraft) {
      history = historyDraft
      clearComposerDraft(scope, expected)
    },
  })

  expect(await submit()).toBe("submitted")
  expect(history).toEqual({ ...snapshot, text: "inspect this" })
  expect(composerDraft(key)).toEqual({ text: "", mentions: [], staged: [] })
})

test("failed composer submissions retain text, mentions, and attachment-only drafts", async () => {
  const { createComposerSubmit } = await import("../src/ui/composer-submit")
  const { composerDraft, composerScope, setComposerDraft } = await import("../src/state/composer")
  const drafts = [
    { text: "retry with context", mentions: ["src/retry.ts"], staged: [attachment] },
    { text: "", mentions: [], staged: [attachment] },
  ]

  for (const [index, snapshot] of drafts.entries()) {
    const key = composerScope(`failed-${index}`, "failed-workspace")
    setComposerDraft(key, snapshot)
    const submit = createComposerSubmit({
      scope: () => key,
      session: () => `failed-${index}`,
      workspace: () => ({ id: "failed-workspace", name: "Failed", path: "C:\\failed" }),
      online: () => true,
      draft: composerDraft,
      prepare: () => ({}),
      transform: async ({ text }) => text,
      newSession: async () => undefined,
      sessionScope: (id) => composerScope(id),
      migrateDraft: () => undefined,
      selectSession: () => undefined,
      sessionCreated: () => undefined,
      send: async () => ({ ok: false, error: "rejected" }),
      admitted: () => undefined,
    })

    expect(await submit()).toBe("failed")
    expect(composerDraft(key)).toBe(snapshot)
  }
})

test("edits made while a prompt is pending survive successful admission", async () => {
  const { createComposerSubmit } = await import("../src/ui/composer-submit")
  const { clearComposerDraft, composerDraft, composerScope, patchComposerDraft, setComposerDraft } = await import(
    "../src/state/composer"
  )
  const key = composerScope("editing-session", "editing-workspace")
  const snapshot = { text: "first prompt", mentions: [], staged: [attachment] }
  setComposerDraft(key, snapshot)
  const started = deferred<void>()
  const admission = deferred<void>()
  const submit = createComposerSubmit({
    scope: () => key,
    session: () => "editing-session",
    workspace: () => ({ id: "editing-workspace", name: "Editing", path: "C:\\editing" }),
    online: () => true,
    draft: composerDraft,
    prepare: () => ({}),
    transform: async ({ text }) => text,
    newSession: async () => undefined,
    sessionScope: (id) => composerScope(id),
    migrateDraft: () => undefined,
    selectSession: () => undefined,
    sessionCreated: () => undefined,
    send: async () => {
      started.resolve()
      await admission.promise
      return { ok: true }
    },
    admitted: (scope, expected) => void clearComposerDraft(scope, expected),
  })

  const pending = submit()
  await started.promise
  patchComposerDraft(key, { text: "newer unsent edit", mentions: ["src/new.ts"] })
  admission.resolve()
  expect(await pending).toBe("submitted")
  expect(composerDraft(key)).toEqual({
    text: "newer unsent edit",
    mentions: ["src/new.ts"],
    staged: [attachment],
  })
})

test("SDK rejection and thrown sends preserve newer composer edits", async () => {
  const { createComposerSubmit } = await import("../src/ui/composer-submit")
  const { clearComposerDraft, composerDraft, composerScope, patchComposerDraft, setComposerDraft } = await import(
    "../src/state/composer"
  )
  const newerAttachment = { ...attachment, id: "file-2", filename: "newer.png" }

  for (const failure of ["rejected", "thrown"] as const) {
    const key = composerScope(`${failure}-newer-session`, "newer-workspace")
    const snapshot = { text: "first prompt", mentions: ["src/first.ts"], staged: [attachment] }
    const newer = {
      text: "newer unsent edit",
      mentions: ["src/newer.ts"],
      staged: [attachment, newerAttachment],
    }
    setComposerDraft(key, snapshot)
    const started = deferred<void>()
    const finish = deferred<void>()
    const submit = createComposerSubmit({
      scope: () => key,
      session: () => `${failure}-newer-session`,
      workspace: () => ({ id: "newer-workspace", name: "Newer", path: "C:\\newer" }),
      online: () => true,
      draft: composerDraft,
      prepare: () => ({}),
      transform: async ({ text }) => text,
      newSession: async () => undefined,
      sessionScope: (id) => composerScope(id),
      migrateDraft: () => undefined,
      selectSession: () => undefined,
      sessionCreated: () => undefined,
      send: async () => {
        started.resolve()
        await finish.promise
        if (failure === "thrown") throw new Error("transport closed")
        return { ok: false, error: "engine rejected the request" }
      },
      admitted: (scope, expected) => void clearComposerDraft(scope, expected),
    })

    const pending = submit()
    await started.promise
    patchComposerDraft(key, newer)
    finish.resolve()
    expect(await pending).toBe("failed")
    expect(composerDraft(key)).toEqual(newer)
  }
})

test("new-session submits are single-flight during transform and session creation", async () => {
  const { createComposerSubmissionGuard, createComposerSubmit } = await import("../src/ui/composer-submit")
  const { clearComposerDraft, composerDraft, composerScope, migrateComposerDraft, setComposerDraft } = await import(
    "../src/state/composer"
  )

  for (const delayed of ["transform", "session"] as const) {
    const workspaceId = `single-flight-${delayed}`
    const source = composerScope(null, workspaceId)
    const target = composerScope(`created-${delayed}`)
    setComposerDraft(source, { text: "create one thread", mentions: [], staged: [] })
    let currentScope = source
    let transforms = 0
    let sessions = 0
    let sends = 0
    const gate = deferred<void>()
    const guard = createComposerSubmissionGuard()
    const submit = createComposerSubmit(
      {
        scope: () => currentScope,
        session: () => null,
        workspace: () => ({ id: workspaceId, name: workspaceId, path: `C:\\${workspaceId}` }),
        online: () => true,
        draft: composerDraft,
        prepare: () => ({}),
        transform: async ({ text }) => {
          transforms++
          if (delayed === "transform") await gate.promise
          return text
        },
        newSession: async () => {
          sessions++
          if (delayed === "session") await gate.promise
          return { id: `created-${delayed}`, discard: async () => undefined }
        },
        sessionScope: (id) => composerScope(id),
        migrateDraft: migrateComposerDraft,
        selectSession: () => (currentScope = target),
        sessionCreated: () => undefined,
        send: async () => {
          sends++
          return { ok: true }
        },
        admitted: (scope, expected) => void clearComposerDraft(scope, expected),
      },
      guard,
    )

    const first = submit()
    expect(await submit()).toBe("ignored")
    expect(transforms).toBe(1)
    expect(sessions).toBe(delayed === "session" ? 1 : 0)
    gate.resolve()
    expect(await first).toBe("submitted")
    expect(sessions).toBe(1)
    expect(sends).toBe(1)
    expect(guard.has(source)).toBeFalse()
    expect(guard.has(target)).toBeFalse()
    expect(composerDraft(source).text).toBe("")
    expect(composerDraft(target).text).toBe("")
  }
})

test("new-session submit preserves navigation and drafts when creation resolves in another workspace", async () => {
  const { createComposerSubmissionGuard, createComposerSubmit } = await import("../src/ui/composer-submit")
  const { composerDraft, composerScope, migrateComposerDraft, setComposerDraft } = await import(
    "../src/state/composer"
  )
  const workspaceA = { id: "workspace-a", name: "A", path: "C:\\a" }
  const workspaceB = { id: "workspace-b", name: "B", path: "C:\\b" }
  const sourceA = composerScope(null, workspaceA.id)
  const sourceB = composerScope(null, workspaceB.id)
  const draftA = { text: "send from A", mentions: ["src/a.ts"], staged: [attachment] }
  const draftB = { text: "keep B", mentions: ["src/b.ts"], staged: [] }
  setComposerDraft(sourceA, draftA)
  setComposerDraft(sourceB, draftB)
  let currentScope = sourceA
  let currentWorkspace = workspaceA
  let selections = 0
  let createdEvents = 0
  let sends = 0
  let discards = 0
  const started = deferred<void>()
  const created = deferred<{ id: string; discard: () => Promise<void> }>()
  const guard = createComposerSubmissionGuard()
  const submit = createComposerSubmit(
    {
      scope: () => currentScope,
      session: () => null,
      workspace: () => currentWorkspace,
      online: () => true,
      draft: composerDraft,
      prepare: () => ({}),
      transform: async ({ text }) => text,
      newSession: async () => {
        started.resolve()
        return created.promise
      },
      sessionScope: (id) => composerScope(id),
      migrateDraft: migrateComposerDraft,
      selectSession: () => selections++,
      sessionCreated: () => createdEvents++,
      send: async () => {
        sends++
        return { ok: true }
      },
      admitted: () => undefined,
    },
    guard,
  )

  const pending = submit()
  await started.promise
  currentScope = sourceB
  currentWorkspace = workspaceB
  created.resolve({ id: "created-in-a", discard: async () => void discards++ })

  expect(await pending).toBe("ignored")
  expect(selections).toBe(0)
  expect(createdEvents).toBe(0)
  expect(sends).toBe(0)
  expect(discards).toBe(1)
  expect(composerDraft(sourceA)).toBe(draftA)
  expect(composerDraft(sourceB)).toBe(draftB)
  expect(composerDraft(composerScope("created-in-a")).text).toBe("")
  expect(guard.has(sourceA)).toBeFalse()
})

test("existing-session submits are single-flight while prompt admission is pending", async () => {
  const { createComposerSubmissionGuard, createComposerSubmit } = await import("../src/ui/composer-submit")
  const { clearComposerDraft, composerDraft, composerScope, setComposerDraft } = await import("../src/state/composer")
  const key = composerScope("pending-send-session", "pending-send-workspace")
  setComposerDraft(key, { text: "send once", mentions: [], staged: [] })
  const admission = deferred<void>()
  const started = deferred<void>()
  const guard = createComposerSubmissionGuard()
  let sends = 0
  const submit = createComposerSubmit(
    {
      scope: () => key,
      session: () => "pending-send-session",
      workspace: () => ({ id: "pending-send-workspace", name: "Pending", path: "C:\\pending" }),
      online: () => true,
      draft: composerDraft,
      prepare: () => ({}),
      transform: async ({ text }) => text,
      newSession: async () => undefined,
      sessionScope: (id) => composerScope(id),
      migrateDraft: () => undefined,
      selectSession: () => undefined,
      sessionCreated: () => undefined,
      send: async () => {
        sends++
        started.resolve()
        await admission.promise
        return { ok: true }
      },
      admitted: (scope, expected) => void clearComposerDraft(scope, expected),
    },
    guard,
  )

  const first = submit()
  await started.promise
  expect(guard.has(key)).toBeTrue()
  expect(await submit()).toBe("ignored")
  expect(sends).toBe(1)
  admission.resolve()
  expect(await first).toBe("submitted")
  expect(guard.has(key)).toBeFalse()
})

test("failed new-session admission keeps the migrated draft in the created session scope", async () => {
  const { createComposerSubmissionGuard, createComposerSubmit } = await import("../src/ui/composer-submit")
  const { composerDraft, composerScope, migrateComposerDraft, patchComposerDraft, setComposerDraft } = await import(
    "../src/state/composer"
  )
  const workspaceId = "migration-workspace"
  const source = composerScope(null, workspaceId)
  const target = composerScope("migrated-session")
  const snapshot = { text: "keep after failure", mentions: ["src/migrate.ts"], staged: [attachment] }
  const newer = { ...snapshot, text: "newer in created session", mentions: ["src/newer.ts"] }
  setComposerDraft(source, snapshot)
  let currentScope = source
  const started = deferred<void>()
  const admission = deferred<void>()
  const guard = createComposerSubmissionGuard()
  const submit = createComposerSubmit(
    {
      scope: () => currentScope,
      session: () => null,
      workspace: () => ({ id: workspaceId, name: "Migration", path: "C:\\migration" }),
      online: () => true,
      draft: composerDraft,
      prepare: () => ({}),
      transform: async ({ text }) => text,
      newSession: async () => ({ id: "migrated-session", discard: async () => undefined }),
      sessionScope: (id) => composerScope(id),
      migrateDraft: migrateComposerDraft,
      selectSession: () => (currentScope = target),
      sessionCreated: () => undefined,
      send: async () => {
        started.resolve()
        await admission.promise
        return { ok: false, error: "rejected" }
      },
      admitted: () => undefined,
    },
    guard,
  )

  const pending = submit()
  await started.promise
  expect(composerDraft(source).text).toBe("")
  expect(composerDraft(target)).toBe(snapshot)
  patchComposerDraft(target, newer)
  admission.resolve()
  expect(await pending).toBe("failed")
  expect(composerDraft(target)).toEqual(newer)
  expect(guard.has(source)).toBeFalse()
  expect(guard.has(target)).toBeFalse()
})

test("composer submission guard releases after thrown failures", async () => {
  const { createComposerSubmissionGuard, createComposerSubmit } = await import("../src/ui/composer-submit")
  const { clearComposerDraft, composerDraft, composerScope, setComposerDraft } = await import("../src/state/composer")
  const key = composerScope("release-session", "release-workspace")
  setComposerDraft(key, { text: "retry me", mentions: [], staged: [] })
  const guard = createComposerSubmissionGuard()
  let sends = 0
  const submit = createComposerSubmit(
    {
      scope: () => key,
      session: () => "release-session",
      workspace: () => ({ id: "release-workspace", name: "Release", path: "C:\\release" }),
      online: () => true,
      draft: composerDraft,
      prepare: () => ({}),
      transform: async ({ text }) => text,
      newSession: async () => undefined,
      sessionScope: (id) => composerScope(id),
      migrateDraft: () => undefined,
      selectSession: () => undefined,
      sessionCreated: () => undefined,
      send: async () => {
        if (++sends === 1) throw new Error("transport closed")
        return { ok: true }
      },
      admitted: (scope, expected) => void clearComposerDraft(scope, expected),
    },
    guard,
  )

  expect(await submit()).toBe("failed")
  expect(guard.has(key)).toBeFalse()
  expect(composerDraft(key).text).toBe("retry me")
  expect(await submit()).toBe("submitted")
  expect(sends).toBe(2)
  expect(composerDraft(key).text).toBe("")
})

test("engine send reports admission, SDK rejection, and thrown transport failures", async () => {
  const { createActions } = await import("../src/engine/actions")
  const { createEngineState } = await import("../src/engine/store")
  const [state, set] = createEngineState()
  let result: { data?: unknown; error?: unknown } | Error = { data: {} }
  let request: unknown
  const client = {
    session: {
      promptAsync: async (input: unknown) => {
        request = input
        if (result instanceof Error) throw result
        return result
      },
    },
  }
  const actions = createActions(() => client as never, state, set, () => undefined)
  const options = {
    model: { providerID: "anthropic", modelID: "claude-opus-5" },
    agent: "build",
    variant: "high",
    files: [attachment],
  }

  expect(await actions.send("send-session", "", options)).toEqual({ ok: true })
  expect(state.errors["send-session"]).toBeUndefined()
  expect(request).toMatchObject({
    path: { id: "send-session" },
    body: {
      model: { providerID: "anthropic", modelID: "claude-opus-5" },
      agent: "build",
      variant: "high",
      parts: [{ type: "file" }],
    },
  })

  result = { error: { data: { message: "quota exceeded" } } }
  expect(await actions.send("send-session", "retry", options)).toEqual({
    ok: false,
    error: "Prompt failed: quota exceeded",
  })
  expect(state.errors["send-session"]).toBe("Prompt failed: quota exceeded")

  result = new Error("transport unavailable")
  expect(await actions.send("send-session", "retry", options)).toEqual({
    ok: false,
    error: "Prompt failed: transport unavailable",
  })
  expect(state.errors["send-session"]).toBe("Prompt failed: transport unavailable")
})

test("question navigation selects the normalized owning workspace before its session", async () => {
  const { selectOwningSession } = await import("../src/ui/composer")
  const workspaces = [
    { id: "workspace-a", path: "C:\\work\\alpha" },
    { id: "workspace-b", path: "D:\\work\\beta\\" },
  ]
  const calls: string[] = []

  selectOwningSession(
    "session-b",
    "d:/WORK/beta",
    workspaces,
    "workspace-a",
    (id) => calls.push(`workspace:${id}`),
    (id) => calls.push(`session:${id}`),
  )

  expect(calls).toEqual(["workspace:workspace-b", "session:session-b"])
})
