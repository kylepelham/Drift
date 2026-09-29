import { afterEach, beforeEach, expect, mock, test } from "bun:test"
import { backendInvoke, backendRoute } from "../src/backend"
import { isNarrowWidth, navigationHash, parseNavigationHash } from "../src/state/navigation"
import { nextRemoteAccessEnabled, normalizeLinkCode, remoteStatusTone, type RemoteAccessStatus } from "../src/state/remote-access"
import { remoteEngineBase, remoteRuntimeFrom, runtimeNameFrom } from "../src/runtime"

test("remote runtime uses the same-origin engine gateway", () => {
  const remote = { pathname: "/companion", origin: "https://192.168.1.8:41718" }
  const desktop = { pathname: "/", origin: "http://localhost:5180" }
  expect(remoteRuntimeFrom(remote)).toBe(true)
  expect(remoteRuntimeFrom(desktop)).toBe(false)
  expect(remoteEngineBase(remote)).toBe("https://192.168.1.8:41718/engine")
  expect(remoteEngineBase(desktop)).toBeUndefined()
})

test("dynamic viewport sizing is limited to the remote runtime", () => {
  expect(runtimeNameFrom({ pathname: "/companion", origin: "https://192.168.1.20:41718" })).toBe("remote")
  expect(runtimeNameFrom({ pathname: "/", origin: "tauri://localhost" })).toBe("desktop")
})

test("host backend routing prefers Tauri and otherwise uses remote RPC", () => {
  expect(backendRoute(true, true)).toBe("tauri")
  expect(backendRoute(false, true)).toBe("rpc")
  expect(backendRoute(false, false)).toBe("browser")
})

let fetchMock: ReturnType<typeof mock>
let replaceMock: ReturnType<typeof mock>
const globalNames = ["window", "location", "fetch"] as const
let originalGlobals: (PropertyDescriptor | undefined)[]

function setGlobal(name: (typeof globalNames)[number], value: unknown) {
  Object.defineProperty(globalThis, name, { configurable: true, writable: true, value })
}

beforeEach(() => {
  originalGlobals = globalNames.map((name) => Object.getOwnPropertyDescriptor(globalThis, name))
  fetchMock = mock(async (url: string) => Response.json(url === "/auth/me" ? { id: "device" } : { error: "remote access credentials changed" }, { status: url === "/auth/me" ? 200 : 401 }))
  replaceMock = mock(() => {})
  const location = { ...new URL("https://192.168.1.8:41718/companion"), pathname: "/companion", origin: "https://192.168.1.8:41718", replace: replaceMock }
  setGlobal("fetch", fetchMock)
  setGlobal("location", location)
  setGlobal("window", { location })
})

afterEach(() => {
  for (const [index, name] of globalNames.entries()) {
    const descriptor = originalGlobals[index]
    if (descriptor) Object.defineProperty(globalThis, name, descriptor)
    else delete (globalThis as Record<string, unknown>)[name]
  }
})

test("remote RPC 401 confirms the current session before redirecting", async () => {
  const invoke = backendInvoke()!
  await expect(invoke("read_file_preview", { path: "C:/tmp/a.png" })).rejects.toThrow("remote access credentials changed")
  expect(fetchMock.mock.calls.map(([url]) => url)).toEqual(["/api/invoke", "/auth/me"])
  expect(replaceMock).not.toHaveBeenCalled()
})

test("remote RPC redirects only when the current session is invalid", async () => {
  fetchMock.mockImplementation(async (url: string) => Response.json(url === "/auth/me" ? {} : { error: "remote access credentials changed" }, { status: 401 }))
  const invoke = backendInvoke()!
  await expect(invoke("read_file_preview", { path: "C:/tmp/a.png" })).rejects.toThrow("remote access credentials changed")
  expect(fetchMock.mock.calls.map(([url]) => url)).toEqual(["/api/invoke", "/auth/me"])
  expect(replaceMock.mock.calls).toEqual([["/companion"]])
})

test("responsive navigation state round-trips and uses the narrow breakpoint", () => {
  expect(isNarrowWidth(390)).toBe(true)
  expect(isNarrowWidth(720)).toBe(false)
  const hash = navigationHash({ workspace: "project A", session: "ses_1", overlay: "drawer" })
  expect(parseNavigationHash(hash)).toEqual({ workspace: "project A", session: "ses_1", overlay: "drawer" })
})

test("mobile layout keeps scrolling inside the transcript and drawer", async () => {
  const app = await Bun.file("src/app.tsx").text()
  const css = await Bun.file("src/styles/app.css").text()
  const parts = await Bun.file("src/ui/parts.tsx").text()
  const markdown = await Bun.file("src/ui/markdown.tsx").text()
  const composer = await Bun.file("src/ui/composer.tsx").text()
  const sidebar = await Bun.file("src/ui/sidebar.tsx").text()
  const workspaces = await Bun.file("src/ui/workspaces.tsx").text()

  expect(css).toMatch(/html,\s*body,\s*#root\s*\{[^}]*overflow: hidden/s)
  expect(css).toMatch(/\.app-shell\s*\{[^}]*overflow: hidden/s)
  expect(css).toMatch(/\.transcript-scroll\s*\{[^}]*overscroll-behavior-y: contain/s)
  expect(css).toMatch(/@media \(max-width: 719px\)[\s\S]*\.chat-pane-covered\s*\{[^}]*visibility: hidden/s)
  expect(await Bun.file("src/ui/debug.tsx").text()).toContain("debug-panel-scroll")
  expect(await Bun.file("src/app.tsx").text()).toContain('debugPanelOpen() && !!selectedSession()')
  expect(css).toMatch(/\.composer-dock\s*\{[^}]*padding-top: 1rem/s)
  expect(css).toMatch(/\.composer-dock::before\s*\{[^}]*inset: -2\.75rem 0 auto;[^}]*linear-gradient/s)
  expect(css).toMatch(/\.dock-card\s*\{[^}]*box-shadow:/s)
  expect(app.match(/min-h-0 min-w-0 flex-1[^\"]*overflow-hidden/g)).toHaveLength(2)
  expect(parts.match(/transcript-tool-output/g)?.length).toBeGreaterThanOrEqual(5)
  expect(markdown).toContain("transcript-tool-output code-view code-stream")
  expect(css).toMatch(/@media \(max-width: 719px\)[\s\S]*\.transcript-scroll\.transcript-scroll-active \.transcript-tool-output\s*\{[^}]*pointer-events: none/s)
  expect(css).not.toMatch(/\.transcript-tool-output\s*\{[^}]*max-height: none/s)
  expect(await Bun.file("src/ui/chat.tsx").text()).toContain('classList.add("transcript-scroll-active")')
  expect(composer).toContain('class="composer-options relative flex min-w-0 flex-1')
  expect(composer).not.toContain("composer-options flex min-w-0 flex-1 items-center gap-1 overflow-hidden")
  expect(composer).toContain('class="composer-action-buttons ml-auto flex shrink-0')
  // The composer keeps its compact single-row desktop layout on mobile: pickers shrink and
  // truncate instead of stretching into full-width rows, and buttons keep their desktop size.
  expect(css).toMatch(/\.composer-options > \.picker-control\s*\{[^}]*position: static;[^}]*min-width: 0;[^}]*flex: 0 1 auto/s)
  expect(css).not.toMatch(/\.composer-actions\s*\{[^}]*flex-wrap: wrap/s)
  expect(css).not.toMatch(/flex-basis: 100%/)
  expect(css).toMatch(/\.composer-actions button,\s*\.composer-actions \[role="button"\]\s*\{[^}]*min-height: 0/s)
  // The drawer must not add safe-area padding: the wrapper webview already sits below the
  // status bar, so env(safe-area-inset-top) doubles into a large blank band at the top.
  expect(css).not.toMatch(/\.app-sidebar\s*\{[^}]*safe-area-inset-top/s)
  expect(css).toMatch(/\.app-sidebar\s*\{[^}]*visibility: hidden;[^}]*visibility 0s linear 180ms/s)
  expect(css).toMatch(/\.app-sidebar\.mobile-sidebar-open\s*\{[^}]*visibility: visible/s)
  expect(css).toMatch(/\.app-sidebar-scroll\s*\{[^}]*touch-action: pan-y/s)
  expect(sidebar).toContain('event.target.closest("[data-sidebar-navigation]")')
  expect(workspaces).toContain('class="group sticky top-0')
})

test("remote settings state distinguishes online, offline, and error", () => {
  const base: RemoteAccessStatus = {
    enabled: true,
    listening: true,
    port: 41718,
    discoveryPort: 41717,
    urls: [],
    devices: [],
    pendingLinks: [],
    certificateFingerprint: "AB:CD",
  }
  expect(remoteStatusTone(base)).toBe("online")
  expect(remoteStatusTone({ ...base, listening: false })).toBe("offline")
  expect(remoteStatusTone({ ...base, error: "busy" })).toBe("error")
  expect(nextRemoteAccessEnabled(base)).toBeFalse()
  expect(nextRemoteAccessEnabled({ ...base, enabled: false })).toBeTrue()
})

test("remote access toggle has one state transition helper", async () => {
  const section = await Bun.file("src/ui/settings-remote-access.tsx").text()
  expect(section).toContain("onChange={() => void setRemoteAccess(nextRemoteAccessEnabled(status()))}")
  expect(section).not.toContain("onClick={() => void setRemoteAccess")
  expect(section).not.toContain("remote-access-card")
})

test("typed link codes ignore case, spaces, and the separator", () => {
  expect(normalizeLinkCode(" abcd-efgh ")).toBe("ABCDEFGH")
  expect(normalizeLinkCode("AB CD EF GH")).toBe("ABCDEFGH")
})

test("the shared access-key flow is gone and management stays desktop-only", async () => {
  const state = await Bun.file("src/state/remote-access.ts").text()
  const section = await Bun.file("src/ui/settings-remote-access.tsx").text()
  for (const source of [state, section]) {
    expect(source).not.toContain("connectionUrls")
    expect(source).not.toContain("rotate")
    expect(source).not.toContain("?token=")
  }
  expect(section).toContain('<Show when={!isRemoteRuntime()} fallback={<ThisDevice />}>')
  expect(await Bun.file("src/backend.ts").text()).toContain('if (response.status === 401 && await remoteSessionExpired()) window.location.replace("/companion")')
})

test("device activity reads as relative time", async () => {
  const { lastSeenLabel } = await import("../src/state/remote-access")
  const now = Date.UTC(2026, 8, 28, 12)
  const english = (at: number) => lastSeenLabel(at, now)
  expect(english(now - 20_000)).toBe(new Intl.RelativeTimeFormat(undefined, { numeric: "auto" }).format(0, "minute"))
  expect(english(now - 5 * 60_000)).toBe(new Intl.RelativeTimeFormat(undefined, { numeric: "auto" }).format(-5, "minute"))
  expect(english(now - 3 * 86_400_000)).toBe(new Intl.RelativeTimeFormat(undefined, { numeric: "auto" }).format(-3, "day"))
})

test("remote settings drop the redundant gateway rows and keep linking in numbered steps", async () => {
  const section = await Bun.file("src/ui/settings-remote-access.tsx").text()
  expect(section).not.toContain("listeningAddress")
  expect(section).not.toContain("drift.remote.encryption.https")
  expect(section).toContain('id="remote-link-code"')
  expect(section.match(/<Step number=\{\d\}>/g)).toHaveLength(3)
})
