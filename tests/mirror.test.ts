import { expect, test } from "bun:test"

const mirrorStorage = new Map<string, string>()
if (!("localStorage" in globalThis)) {
  Object.defineProperty(globalThis, "localStorage", {
    value: {
      getItem: (key: string) => mirrorStorage.get(key) ?? null,
      setItem: (key: string, value: string) => mirrorStorage.set(key, value),
    },
  })
}
localStorage.getItem = (key: string) => mirrorStorage.get(key) ?? null
localStorage.setItem = (key: string, value: string) => mirrorStorage.set(key, value)

const host = {
  schema: 1 as const,
  revision: 4,
  theme: {
    name: "drift-paper" as const,
    custom: { background: "#ffffff", surface: "#f5f5f5", text: "#111111", accent: "#3366cc" },
    uiFont: "Inter",
    codeFont: "Cascadia Code",
    customCss: ".host { color: red; }",
  },
  selection: { workspaceId: "workspace-1", sessionId: "session-1" },
  workspaceOrder: ["workspace-2", "workspace-1"],
}

test("remote bootstrap cache applies host theme, order, and atomic selection before app modules load", async () => {
  const { acceptMirrorSnapshot, registerMirrorApplier } = await import("../src/state/mirror")
  const applied: unknown[] = []
  registerMirrorApplier({
    theme: (theme) => applied.push(theme),
    order: (ids) => applied.push(ids),
    selection: (selection) => applied.push(selection),
  })
  expect(acceptMirrorSnapshot(host, true)).toBeTrue()
  expect(JSON.parse(localStorage.getItem("drift.theme")!)).toBe("drift-paper")
  expect(JSON.parse(localStorage.getItem("drift.workspace")!)).toBe("workspace-1")
  expect(JSON.parse(localStorage.getItem("drift.session")!)).toBe("session-1")
  expect(JSON.parse(localStorage.getItem("drift.workspace.order")!)).toEqual(["workspace-2", "workspace-1"])
  expect(applied.at(-1)).toEqual(host.selection)
  expect(applied).toContainEqual(host.workspaceOrder)
})

test("storage failures do not prevent mirrored state from applying", async () => {
  const { acceptMirrorSnapshot, registerMirrorApplier } = await import("../src/state/mirror")
  const applied: unknown[] = []
  const original = localStorage.setItem
  localStorage.setItem = () => {
    throw new Error("quota exceeded")
  }
  try {
    registerMirrorApplier({
      theme: (theme) => applied.push(theme),
      order: (ids) => applied.push(ids),
      selection: (selection) => applied.push(selection),
    })
    expect(acceptMirrorSnapshot({ ...host, revision: 5 }, true)).toBeTrue()
    expect(applied).toContainEqual(host.theme)
    expect(applied).toContainEqual(host.workspaceOrder)
    expect(applied).toContainEqual(host.selection)
  } finally {
    localStorage.setItem = original
  }
})

test("desktop bootstrap normalizes malformed legacy appearance and selection values", async () => {
  const { acceptMirrorSnapshot, localMirrorSnapshot } = await import("../src/state/mirror")
  mirrorStorage.set("drift.theme", JSON.stringify("legacy-theme"))
  mirrorStorage.set("drift.theme.custom", JSON.stringify({ background: "red", surface: "#123456" }))
  mirrorStorage.set("drift.theme.uiFont", JSON.stringify("界".repeat(300)))
  mirrorStorage.set("drift.theme.customCss", JSON.stringify("x".repeat(20_100)))
  mirrorStorage.set("drift.workspace", JSON.stringify("bad\u0000workspace"))
  mirrorStorage.set("drift.session", JSON.stringify("session-1"))
  mirrorStorage.set("drift.workspace.order", JSON.stringify(["one", "one", "bad\u0000id", ...Array.from({ length: 501 }, (_, index) => `w-${index}`)]))

  const snapshot = localMirrorSnapshot()
  expect(snapshot.theme.name).toBe("drift-dark")
  expect(snapshot.theme.custom).toEqual({ background: "#111318", surface: "#123456", text: "#e8eaf0", accent: "#a78bfa" })
  expect(snapshot.theme.uiFont).toHaveLength(256)
  expect([...snapshot.theme.uiFont]).toHaveLength(256)
  expect(snapshot.theme.customCss).toHaveLength(20_000)
  expect(snapshot.selection).toEqual({ workspaceId: null, sessionId: null })
  expect(snapshot.workspaceOrder).toHaveLength(500)
  expect(new Set(snapshot.workspaceOrder).size).toBe(500)
  acceptMirrorSnapshot(host, true)
})

test("appearance setters truncate Unicode without splitting surrogate pairs", async () => {
  const { customCss, setCustomCss } = await import("../src/state/theme")
  setCustomCss("😀".repeat(20_001))
  expect([...customCss()]).toHaveLength(20_000)
  expect(customCss().endsWith("😀")).toBeTrue()
})

test("mirror ignores duplicate and out-of-order revisions without inbound feedback", async () => {
  const { acceptMirrorSnapshot, shouldAcceptRevision } = await import("../src/state/mirror")
  expect(shouldAcceptRevision(4, 5)).toBeTrue()
  expect(shouldAcceptRevision(4, 4)).toBeFalse()
  expect(shouldAcceptRevision(4, 3)).toBeFalse()
  expect(acceptMirrorSnapshot({ ...host, revision: 4 })).toBeFalse()
  expect(acceptMirrorSnapshot({ ...host, revision: 3 })).toBeFalse()
  expect(localStorage.getItem("drift.workspace")).toBe(JSON.stringify("workspace-1"))
})

test("desktop initializes once while companion only snapshots host state and policy", async () => {
  const { mirrorBootstrapCommand, shellTimeoutBootstrapCommand } = await import("../src/state/mirror")
  expect(mirrorBootstrapCommand(false)).toBe("ui_state_initialize")
  expect(mirrorBootstrapCommand(true)).toBe("ui_state_snapshot")
  expect(shellTimeoutBootstrapCommand(false)).toBe("shell_timeout_initialize")
  expect(shellTimeoutBootstrapCommand(true)).toBe("shell_timeout_snapshot")
})

test("mirrored workspace selection resolves the engine directory without requiring sessions", async () => {
  const { hydratedWorkspaceSelection, workspaceDirectoryForSelection } = await import("../src/state/workspaces")
  const items = [{ id: "workspace-1", path: "C:/work/drift", name: "Drift", icon: "", lastUsed: 1 }]
  expect(workspaceDirectoryForSelection(items, "workspace-1")).toBe("C:/work/drift")
  expect(workspaceDirectoryForSelection(items, null)).toBeNull()
  expect(hydratedWorkspaceSelection(items, "workspace-1")).toBe("workspace-1")
  expect(hydratedWorkspaceSelection(items, "stale-workspace")).toBe("workspace-1")
  expect(hydratedWorkspaceSelection([], "stale-workspace")).toBeNull()
  expect(hydratedWorkspaceSelection(items, null)).toBeNull()
})

test("reselecting the active workspace preserves its mirrored session", async () => {
  const { acceptMirrorSnapshot, currentMirrorSnapshot } = await import("../src/state/mirror")
  const { applyMirroredSession, selectedSession } = await import("../src/state/selection")
  const { activeWorkspaceId, applyMirroredWorkspace, selectWorkspace } = await import("../src/state/workspaces")
  acceptMirrorSnapshot(host, true)
  applyMirroredWorkspace("workspace-1")
  applyMirroredSession("session-1")
  expect(activeWorkspaceId()).toBe("workspace-1")
  selectWorkspace("workspace-1")
  expect(selectedSession()).toBe("session-1")
  expect(currentMirrorSnapshot()?.selection).toEqual(host.selection)
})

test("stale workspace mirror failures are not retried forever", async () => {
  const source = await Bun.file("src/state/mirror.ts").text()
  expect(source).toContain('message === "selected workspace does not exist"')
  expect(source).toMatch(/message === "selected workspace does not exist"[\s\S]*?retry = undefined[\s\S]*?setLiveError\(""\)/)
  expect(source).toContain("queued = { ...retry.patch, ...queued }")
})

test("startup replaces its host placeholder after workspace hydration", async () => {
  const source = await Bun.file("src/main.tsx").text()
  expect(source.indexOf("await workspaces.initWorkspaces()")).toBeLessThan(source.indexOf("render(() => <App />"))
  expect(source.lastIndexOf("root.replaceChildren()")).toBeLessThan(source.indexOf("render(() => <App />"))
  expect(source).toContain("Connecting to Drift host...")
})
