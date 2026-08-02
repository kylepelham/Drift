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
}

test("remote bootstrap cache applies host theme and atomic selection before app modules load", async () => {
  const { acceptMirrorSnapshot, registerMirrorApplier } = await import("../src/state/mirror")
  const applied: unknown[] = []
  registerMirrorApplier({
    theme: (theme) => applied.push(theme),
    selection: (selection) => applied.push(selection),
  })
  expect(acceptMirrorSnapshot(host, true)).toBeTrue()
  expect(JSON.parse(localStorage.getItem("drift.theme")!)).toBe("drift-paper")
  expect(JSON.parse(localStorage.getItem("drift.workspace")!)).toBe("workspace-1")
  expect(JSON.parse(localStorage.getItem("drift.session")!)).toBe("session-1")
  expect(applied.at(-1)).toEqual(host.selection)
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
  const { workspaceDirectoryForSelection } = await import("../src/state/workspaces")
  const items = [{ id: "workspace-1", path: "C:/work/drift", name: "Drift", icon: "", lastUsed: 1 }]
  expect(workspaceDirectoryForSelection(items, "workspace-1")).toBe("C:/work/drift")
  expect(workspaceDirectoryForSelection(items, null)).toBeNull()
})

test("app startup hydrates host workspaces before rendering WorkspaceBinding", async () => {
  const source = await Bun.file("src/main.tsx").text()
  expect(source.indexOf("await workspaces.initWorkspaces()")).toBeLessThan(source.indexOf("render(() => <App />"))
  expect(source).toContain("Connecting to Drift host...")
})
